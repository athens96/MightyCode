//! macOS native voice dictation & Push-to-Talk (`MC-4`): records 16 kHz mono PCM via `AVAudioRecorder` with live dB level metering
//! for the composer waveform, then transcribes either through a local `whisper-cli` model (when configured) or Apple's `Speech.framework`
//! (`SFSpeechRecognizer` + `SFSpeechURLRecognitionRequest`, preferring Apple Silicon on-device recognition and passing workspace file and
//! symbol names via `contextualStrings`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use block2::RcBlock;
use mighty_platform::{PlatformError, Result, SpeechEvent, SpeechPermission, SpeechRecognizer, SpeechRequest, SpeechSession};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2_foundation::{NSArray, NSBundle, NSDictionary, NSError, NSNumber, NSString, NSURL, NSUUID};

#[link(name = "Speech", kind = "framework")]
unsafe extern "C" {}

#[link(name = "AVFAudio", kind = "framework")]
unsafe extern "C" {}

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}

struct SendRetained(Retained<AnyObject>);
// SAFETY: Guarded exclusively by `Mutex<SessionInner>` so concurrent access never happens.
unsafe impl Send for SendRetained {}

struct MacSpeech;

pub fn speech_recognizer() -> Arc<dyn SpeechRecognizer> {
    Arc::new(MacSpeech)
}

/// TCC requires `NSMicrophoneUsageDescription` (and `NSSpeechRecognitionUsageDescription` for Speech.framework) on the main bundle;
/// bare test binaries lack an `Info.plist` and must never trigger TCC assertions.
fn has_bundle_usage_descriptions() -> (bool, bool) {
    objc2::rc::autoreleasepool(|_| {
        let bundle = NSBundle::mainBundle();
        if bundle.bundleIdentifier().is_none() || !bundle.bundlePath().to_string().ends_with(".app") {
            return (false, false);
        }
        let mic_key = NSString::from_str("NSMicrophoneUsageDescription");
        let speech_key = NSString::from_str("NSSpeechRecognitionUsageDescription");
        let has_mic = bundle.objectForInfoDictionaryKey(&mic_key).is_some();
        let has_speech = bundle.objectForInfoDictionaryKey(&speech_key).is_some();
        (has_mic, has_speech)
    })
}

/// Discovers a local Whisper CLI binary + `.bin` GGML model when installed on this Mac.
pub fn local_whisper() -> Option<(PathBuf, PathBuf)> {
    let bin = std::env::var_os("MIGHTY_WHISPER_BIN").map(PathBuf::from).filter(|p| p.is_file()).or_else(|| {
        for candidate in
            ["/opt/homebrew/bin/whisper-cli", "/usr/local/bin/whisper-cli", "/opt/homebrew/bin/whisper-cpp", "/usr/local/bin/whisper-cpp"]
        {
            let path = PathBuf::from(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
        None
    })?;
    let model = std::env::var_os("MIGHTY_WHISPER_MODEL").map(PathBuf::from).filter(|p| p.is_file()).or_else(|| {
        let home = dirs::home_dir()?;
        for rel in [
            ".mightycode/models/ggml-base.bin",
            ".mightycode/models/ggml-small.bin",
            ".cache/whisper/ggml-base.bin",
            ".cache/whisper/ggml-small.bin",
        ] {
            let candidate = home.join(rel);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    })?;
    Some((bin, model))
}

/// `AVCaptureDevice` authorization status for `"soun"` (`0 = NotDetermined`, `1 = Restricted`, `2 = Denied`, `3 = Authorized`).
fn mic_auth_status() -> isize {
    objc2::rc::autoreleasepool(|_| {
        let Some(cls) = AnyClass::get(c"AVCaptureDevice") else { return 1 };
        let media = NSString::from_str("soun");
        unsafe { msg_send![cls, authorizationStatusForMediaType: &*media] }
    })
}

/// `SFSpeechRecognizer` authorization status (`0 = NotDetermined`, `1 = Denied`, `2 = Restricted`, `3 = Authorized`).
fn speech_auth_status() -> isize {
    objc2::rc::autoreleasepool(|_| {
        let Some(cls) = AnyClass::get(c"SFSpeechRecognizer") else { return 2 };
        unsafe { msg_send![cls, authorizationStatus] }
    })
}

fn request_mic_access(done: impl FnOnce(bool) + Send + 'static) {
    objc2::rc::autoreleasepool(|_| {
        let Some(cls) = AnyClass::get(c"AVCaptureDevice") else {
            done(false);
            return;
        };
        let cell = Mutex::new(Some(done));
        let block = RcBlock::new(move |granted: Bool| {
            if let Some(cb) = cell.lock().unwrap_or_else(|p| p.into_inner()).take() {
                cb(granted.as_bool());
            }
        });
        let media = NSString::from_str("soun");
        unsafe {
            let _: () = msg_send![cls, requestAccessForMediaType: &*media, completionHandler: &*block];
        }
    });
}

fn request_speech_access(done: impl FnOnce(bool) + Send + 'static) {
    objc2::rc::autoreleasepool(|_| {
        let Some(cls) = AnyClass::get(c"SFSpeechRecognizer") else {
            done(false);
            return;
        };
        let cell = Mutex::new(Some(done));
        let block = RcBlock::new(move |status: isize| {
            if let Some(cb) = cell.lock().unwrap_or_else(|p| p.into_inner()).take() {
                cb(status == 3);
            }
        });
        unsafe {
            let _: () = msg_send![cls, requestAuthorization: &*block];
        }
    });
}

fn create_wav_recorder(wav_path: &Path) -> std::result::Result<SendRetained, String> {
    objc2::rc::autoreleasepool(|_| {
        let Some(cls) = AnyClass::get(c"AVAudioRecorder") else {
            return Err("AVAudioRecorder is unavailable".into());
        };
        let path_str = NSString::from_str(&wav_path.to_string_lossy());
        let url = NSURL::fileURLWithPath(&path_str);
        // Linear PCM ('lpcm' = 0x6C70636D), 16 kHz, 1 channel, 16-bit little-endian signed integers.
        let k_fmt = NSString::from_str("AVFormatIDKey");
        let k_rate = NSString::from_str("AVSampleRateKey");
        let k_ch = NSString::from_str("AVNumberOfChannelsKey");
        let k_bits = NSString::from_str("AVLinearPCMBitDepthKey");
        let k_be = NSString::from_str("AVLinearPCMIsBigEndianKey");
        let k_fl = NSString::from_str("AVLinearPCMIsFloatKey");

        let v_fmt = NSNumber::new_u32(0x6C70_636D);
        let v_rate = NSNumber::new_f64(16_000.0);
        let v_ch = NSNumber::new_i32(1);
        let v_bits = NSNumber::new_i32(16);
        let v_be = NSNumber::new_bool(false);
        let v_fl = NSNumber::new_bool(false);

        let keys: [&NSString; 6] = [&k_fmt, &k_rate, &k_ch, &k_bits, &k_be, &k_fl];
        let vals: [&AnyObject; 6] = [v_fmt.as_ref(), v_rate.as_ref(), v_ch.as_ref(), v_bits.as_ref(), v_be.as_ref(), v_fl.as_ref()];
        let settings = NSDictionary::<NSString, AnyObject>::from_slices(&keys, &vals);
        let mut err: *mut NSError = std::ptr::null_mut();
        let raw: *mut AnyObject = unsafe {
            let alloc: *mut AnyObject = msg_send![cls, alloc];
            msg_send![alloc, initWithURL: &*url, settings: &*settings, error: &mut err]
        };
        let Some(recorder) = (unsafe { Retained::from_raw(raw) }) else {
            let msg = if !err.is_null() {
                unsafe { (*err).localizedDescription().to_string() }
            } else {
                "could not initialize audio recorder".into()
            };
            return Err(msg);
        };
        unsafe {
            let _: () = msg_send![&*recorder, setMeteringEnabled: Bool::YES];
            let started: Bool = msg_send![&*recorder, record];
            if !started.as_bool() {
                return Err("audio recorder did not start".into());
            }
        }
        Ok(SendRetained(recorder))
    })
}

fn db_to_level(db: f32) -> f32 {
    if !db.is_finite() || db <= -55.0 {
        return 0.04;
    }
    ((db + 55.0) / 55.0).clamp(0.04, 1.0)
}

struct SessionInner {
    recorder: Option<SendRetained>,
    recognition_task: Option<SendRetained>,
    whisper_pid: Option<u32>,
    wav_path: PathBuf,
    stopped: bool,
    cancelled: bool,
    finished: bool,
}

struct ActiveMacSession {
    inner: Arc<Mutex<SessionInner>>,
    metering_alive: Arc<AtomicBool>,
    request: SpeechRequest,
    use_whisper: Option<(PathBuf, PathBuf)>,
    has_speech_plist: bool,
    on_event: Arc<dyn Fn(SpeechEvent) + Send + Sync>,
}

impl ActiveMacSession {
    fn start_recording(self: &Arc<Self>) {
        let wav_path = {
            let guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            if guard.cancelled {
                return;
            }
            guard.wav_path.clone()
        };
        let recorder = match create_wav_recorder(&wav_path) {
            Ok(rec) => rec,
            Err(error) => {
                (self.on_event)(SpeechEvent::Error(error));
                return;
            }
        };
        {
            let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            if guard.cancelled {
                objc2::rc::autoreleasepool(|_| unsafe {
                    let _: () = msg_send![&*recorder.0, stop];
                });
                let _ = std::fs::remove_file(&wav_path);
                return;
            }
            guard.recorder = Some(recorder);
        }

        let session = Arc::downgrade(self);
        let alive = self.metering_alive.clone();
        std::thread::Builder::new()
            .name("mighty-voice-meter".into())
            .spawn(move || {
                while alive.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(65));
                    let Some(strong) = session.upgrade() else { break };
                    let level = {
                        let guard = strong.inner.lock().unwrap_or_else(|p| p.into_inner());
                        if guard.stopped || guard.cancelled {
                            break;
                        }
                        let Some(rec) = &guard.recorder else { break };
                        objc2::rc::autoreleasepool(|_| unsafe {
                            let _: () = msg_send![&*rec.0, updateMeters];
                            let db: f32 = msg_send![&*rec.0, averagePowerForChannel: 0usize];
                            db_to_level(db)
                        })
                    };
                    (strong.on_event)(SpeechEvent::Level(level));
                }
            })
            .ok();
    }

    fn transcribe_wav(self: &Arc<Self>, wav_path: PathBuf) {
        if let Some((bin, model)) = &self.use_whisper {
            let bin = bin.clone();
            let model = model.clone();
            let session = self.clone();
            std::thread::Builder::new()
                .name("mighty-whisper".into())
                .spawn(move || {
                    let lang = session.request.language.split('-').next().unwrap_or("auto");
                    let prompt = session.request.hints.iter().take(60).cloned().collect::<Vec<_>>().join(", ");
                    #[allow(
                        clippy::disallowed_methods,
                        reason = "macOS std::process::Command sets FD_CLOEXEC; spawns optional local whisper-cli"
                    )]
                    let mut cmd = std::process::Command::new(&bin);
                    cmd.arg("-m").arg(&model).arg("-f").arg(&wav_path).arg("-nt").arg("-np");
                    if !lang.is_empty() {
                        cmd.arg("-l").arg(lang);
                    }
                    if !prompt.is_empty() {
                        cmd.arg("--prompt").arg(prompt);
                    }
                    cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
                    let Ok(child) = cmd.spawn() else {
                        let _ = std::fs::remove_file(&wav_path);
                        (session.on_event)(SpeechEvent::Error("failed to launch whisper-cli".into()));
                        return;
                    };
                    {
                        let mut guard = session.inner.lock().unwrap_or_else(|p| p.into_inner());
                        if guard.cancelled {
                            unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
                            let _ = std::fs::remove_file(&wav_path);
                            return;
                        }
                        guard.whisper_pid = Some(child.id());
                    }
                    let output = child.wait_with_output();
                    let _ = std::fs::remove_file(&wav_path);
                    let mut guard = session.inner.lock().unwrap_or_else(|p| p.into_inner());
                    guard.whisper_pid = None;
                    if guard.cancelled || guard.finished {
                        return;
                    }
                    guard.finished = true;
                    drop(guard);
                    match output {
                        Ok(out) if out.status.success() => {
                            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                            (session.on_event)(SpeechEvent::Final { text, engine: "whisper-cli".into() });
                        }
                        _ => {
                            (session.on_event)(SpeechEvent::Error("whisper transcription failed".into()));
                        }
                    }
                })
                .ok();
            return;
        }

        if !self.has_speech_plist {
            let _ = std::fs::remove_file(&wav_path);
            (self.on_event)(SpeechEvent::Error("speech recognition requires MightyCode.app bundle".into()));
            return;
        }

        let session = self.clone();
        let run_wav = wav_path.clone();
        let run = move || {
            let wav_path = run_wav;
            objc2::rc::autoreleasepool(|_| {
                let (Some(rec_cls), Some(req_cls), Some(loc_cls)) =
                    (AnyClass::get(c"SFSpeechRecognizer"), AnyClass::get(c"SFSpeechURLRecognitionRequest"), AnyClass::get(c"NSLocale"))
                else {
                    let _ = std::fs::remove_file(&wav_path);
                    (session.on_event)(SpeechEvent::Error("Speech.framework is unavailable".into()));
                    return;
                };

                let lang_tag = if session.request.language.is_empty() { "ko-KR" } else { &session.request.language };
                let locale_id = NSString::from_str(lang_tag);
                let recognizer: Option<Retained<AnyObject>> = unsafe {
                    let loc_alloc: *mut AnyObject = msg_send![loc_cls, alloc];
                    let loc_raw: *mut AnyObject = msg_send![loc_alloc, initWithLocaleIdentifier: &*locale_id];
                    let locale = Retained::from_raw(loc_raw);
                    let rec_alloc: *mut AnyObject = msg_send![rec_cls, alloc];
                    let rec_raw: *mut AnyObject = match &locale {
                        Some(loc) => msg_send![rec_alloc, initWithLocale: &**loc],
                        None => msg_send![rec_alloc, init],
                    };
                    Retained::from_raw(rec_raw).or_else(|| {
                        let fallback_alloc: *mut AnyObject = msg_send![rec_cls, alloc];
                        let fallback_raw: *mut AnyObject = msg_send![fallback_alloc, init];
                        Retained::from_raw(fallback_raw)
                    })
                };
                let Some(recognizer) = recognizer else {
                    let _ = std::fs::remove_file(&wav_path);
                    (session.on_event)(SpeechEvent::Error("speech recognizer unavailable for locale".into()));
                    return;
                };

                let path_str = NSString::from_str(&wav_path.to_string_lossy());
                let url = NSURL::fileURLWithPath(&path_str);
                let req_obj: Option<Retained<AnyObject>> = unsafe {
                    let alloc: *mut AnyObject = msg_send![req_cls, alloc];
                    let raw: *mut AnyObject = msg_send![alloc, initWithURL: &*url];
                    Retained::from_raw(raw)
                };
                let Some(req_obj) = req_obj else {
                    let _ = std::fs::remove_file(&wav_path);
                    (session.on_event)(SpeechEvent::Error("could not create speech request".into()));
                    return;
                };

                let ns_hints: Vec<Retained<NSString>> = session.request.hints.iter().take(100).map(|h| NSString::from_str(h)).collect();
                let hint_refs: Vec<&NSString> = ns_hints.iter().map(|s| &**s).collect();
                let hints_array = NSArray::from_slice(&hint_refs);

                unsafe {
                    let _: () = msg_send![&*req_obj, setShouldReportPartialResults: Bool::YES];
                    let _: () = msg_send![&*req_obj, setContextualStrings: &*hints_array];
                    let on_device: Bool = msg_send![&*recognizer, supportsOnDeviceRecognition];
                    if on_device.as_bool() {
                        let _: () = msg_send![&*req_obj, setRequiresOnDeviceRecognition: Bool::YES];
                    }
                }

                let callback_session = session.clone();
                let wav_cleanup = wav_path.clone();
                let handler = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
                    if !result.is_null() {
                        let (formatted, is_final) = unsafe {
                            let best: *mut AnyObject = msg_send![result, bestTranscription];
                            let text_ptr: *mut NSString =
                                if best.is_null() { std::ptr::null_mut() } else { msg_send![best, formattedString] };
                            let text = if text_ptr.is_null() { String::new() } else { (*text_ptr).to_string() };
                            let fin: Bool = msg_send![result, isFinal];
                            (text, fin.as_bool())
                        };
                        if is_final {
                            let _ = std::fs::remove_file(&wav_cleanup);
                            let mut guard = callback_session.inner.lock().unwrap_or_else(|p| p.into_inner());
                            if guard.cancelled || guard.finished {
                                return;
                            }
                            guard.finished = true;
                            guard.recognition_task = None;
                            drop(guard);
                            (callback_session.on_event)(SpeechEvent::Final { text: formatted, engine: "apple-speech".into() });
                            return;
                        } else if !formatted.is_empty() {
                            let guard = callback_session.inner.lock().unwrap_or_else(|p| p.into_inner());
                            if !guard.cancelled && !guard.finished {
                                drop(guard);
                                (callback_session.on_event)(SpeechEvent::Partial(formatted));
                            }
                        }
                    }
                    if !error.is_null() {
                        let _ = std::fs::remove_file(&wav_cleanup);
                        let mut guard = callback_session.inner.lock().unwrap_or_else(|p| p.into_inner());
                        if guard.cancelled || guard.finished {
                            return;
                        }
                        guard.finished = true;
                        guard.recognition_task = None;
                        drop(guard);
                        let msg = unsafe { (*error).localizedDescription().to_string() };
                        // Empty silence at the end of a short press-and-hold often reports "No speech detected"; emit empty final cleanly.
                        if msg.to_ascii_lowercase().contains("no speech") {
                            (callback_session.on_event)(SpeechEvent::Final { text: String::new(), engine: "apple-speech".into() });
                        } else {
                            (callback_session.on_event)(SpeechEvent::Error(msg));
                        }
                    }
                });

                let task: Option<Retained<AnyObject>> = unsafe {
                    let raw: *mut AnyObject = msg_send![&*recognizer, recognitionTaskWithRequest: &*req_obj, resultHandler: &*handler];
                    Retained::retain(raw)
                };
                if let Some(task) = task {
                    let mut guard = session.inner.lock().unwrap_or_else(|p| p.into_inner());
                    if guard.cancelled {
                        unsafe {
                            let _: () = msg_send![&*task, cancel];
                        }
                        let _ = std::fs::remove_file(&wav_path);
                    } else {
                        guard.recognition_task = Some(SendRetained(task));
                    }
                }
            });
        };

        if speech_auth_status() == 0 {
            let denied_cb = self.on_event.clone();
            let denied_wav = wav_path.clone();
            request_speech_access(move |granted| {
                if granted {
                    run();
                } else {
                    let _ = std::fs::remove_file(&denied_wav);
                    denied_cb(SpeechEvent::PermissionDenied { kind: "speech" });
                }
            });
        } else if speech_auth_status() == 3 {
            run();
        } else {
            let _ = std::fs::remove_file(&wav_path);
            (self.on_event)(SpeechEvent::PermissionDenied { kind: "speech" });
        }
    }
}

struct MacSpeechSession(Arc<ActiveMacSession>);

impl SpeechSession for MacSpeechSession {
    fn stop(&self) {
        let this = &self.0;
        this.metering_alive.store(false, Ordering::Release);
        let wav_path = {
            let mut guard = this.inner.lock().unwrap_or_else(|p| p.into_inner());
            if guard.stopped || guard.cancelled {
                return;
            }
            guard.stopped = true;
            if let Some(rec) = guard.recorder.take() {
                objc2::rc::autoreleasepool(|_| unsafe {
                    let _: () = msg_send![&*rec.0, stop];
                });
            }
            guard.wav_path.clone()
        };
        (this.on_event)(SpeechEvent::Transcribing);
        this.transcribe_wav(wav_path);
    }

    fn cancel(&self) {
        let this = &self.0;
        this.metering_alive.store(false, Ordering::Release);
        let (recorder, task, pid, wav) = {
            let mut guard = this.inner.lock().unwrap_or_else(|p| p.into_inner());
            if guard.cancelled {
                return;
            }
            guard.cancelled = true;
            guard.stopped = true;
            (guard.recorder.take(), guard.recognition_task.take(), guard.whisper_pid.take(), guard.wav_path.clone())
        };
        objc2::rc::autoreleasepool(|_| unsafe {
            if let Some(rec) = recorder {
                let _: () = msg_send![&*rec.0, stop];
            }
            if let Some(t) = task {
                let _: () = msg_send![&*t.0, cancel];
            }
        });
        if let Some(pid) = pid {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        let _ = std::fs::remove_file(&wav);
        (this.on_event)(SpeechEvent::Cancelled);
    }
}

impl SpeechRecognizer for MacSpeech {
    fn available(&self) -> bool {
        let (has_mic, has_speech) = has_bundle_usage_descriptions();
        has_mic && (has_speech || local_whisper().is_some())
    }

    fn engine_name(&self) -> &'static str {
        if local_whisper().is_some() {
            "whisper-cli"
        } else if self.available() {
            "apple-speech"
        } else {
            "unavailable"
        }
    }

    fn permission(&self) -> SpeechPermission {
        let (has_mic, has_speech) = has_bundle_usage_descriptions();
        if !has_mic {
            return SpeechPermission::Unavailable;
        }
        let mic = mic_auth_status();
        if mic == 1 || mic == 2 {
            return SpeechPermission::Denied;
        }
        if local_whisper().is_none() && has_speech {
            let sp = speech_auth_status();
            if sp == 1 || sp == 2 {
                return SpeechPermission::Denied;
            }
            if mic == 3 && sp == 3 {
                return SpeechPermission::Authorized;
            }
            return SpeechPermission::NotDetermined;
        }
        if mic == 3 { SpeechPermission::Authorized } else { SpeechPermission::NotDetermined }
    }

    fn start(&self, request: SpeechRequest, on_event: Arc<dyn Fn(SpeechEvent) + Send + Sync>) -> Result<Box<dyn SpeechSession>> {
        let (has_mic, has_speech) = has_bundle_usage_descriptions();
        let use_whisper = local_whisper();
        if !has_mic || (!has_speech && use_whisper.is_none()) {
            return Err(PlatformError::unsupported("SpeechRecognizer", "start"));
        }

        let wav_path = std::env::temp_dir().join(format!("mighty-voice-{}.wav", NSUUID::UUID().UUIDString()));
        let active = Arc::new(ActiveMacSession {
            inner: Arc::new(Mutex::new(SessionInner {
                recorder: None,
                recognition_task: None,
                whisper_pid: None,
                wav_path,
                stopped: false,
                cancelled: false,
                finished: false,
            })),
            metering_alive: Arc::new(AtomicBool::new(true)),
            request,
            use_whisper,
            has_speech_plist: has_speech,
            on_event: on_event.clone(),
        });

        let mic = mic_auth_status();
        if mic == 1 || mic == 2 {
            on_event(SpeechEvent::PermissionDenied { kind: "microphone" });
            return Ok(Box::new(MacSpeechSession(active)));
        }
        if mic == 0 {
            let starter = active.clone();
            request_mic_access(move |granted| {
                if granted {
                    starter.start_recording();
                } else {
                    (starter.on_event)(SpeechEvent::PermissionDenied { kind: "microphone" });
                }
            });
        } else {
            active.start_recording();
        }
        Ok(Box::new(MacSpeechSession(active)))
    }

    fn open_permission_settings(&self) -> Result<()> {
        objc2::rc::autoreleasepool(|_| {
            let target = if mic_auth_status() == 3 && speech_auth_status() != 3 {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition"
            } else {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
            };
            let url =
                NSURL::URLWithString(&NSString::from_str(target)).ok_or_else(|| PlatformError::Refused("invalid settings URL".into()))?;
            if objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&url) {
                Ok(())
            } else {
                Err(PlatformError::Refused("System Settings did not open".into()))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decibel_metering_maps_to_bounded_waveform_levels() {
        assert_eq!(db_to_level(-120.0), 0.04);
        assert_eq!(db_to_level(f32::NAN), 0.04);
        assert_eq!(db_to_level(0.0), 1.0);
        let mid = db_to_level(-27.5);
        assert!(mid > 0.45 && mid < 0.55, "{mid}");
    }

    #[test]
    fn unbundled_test_runner_reports_unavailable_without_touching_tcc() {
        let port = speech_recognizer();
        assert!(!port.available());
        assert_eq!(port.permission(), SpeechPermission::Unavailable);
    }
}
