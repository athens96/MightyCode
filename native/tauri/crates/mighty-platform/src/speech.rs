//! Voice dictation & Push-to-Talk port (`MC-4`): OS-native speech recognition (`SFSpeechRecognizer` + `AVAudioRecorder` on macOS)
//! or local Whisper CLI transcription, with real-time audio level metering, contextual code hints, partial/final transcripts,
//! cancellation while recording or transcribing, and privacy-settings deep links.

use std::sync::Arc;

use crate::Result;

/// Current microphone + speech-recognition authorization state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechPermission {
    Authorized,
    Denied,
    NotDetermined,
    Unavailable,
}

impl SpeechPermission {
    pub fn word(self) -> &'static str {
        match self {
            SpeechPermission::Authorized => "authorized",
            SpeechPermission::Denied => "denied",
            SpeechPermission::NotDetermined => "notDetermined",
            SpeechPermission::Unavailable => "unavailable",
        }
    }
}

/// Live events emitted during a voice capture and transcription session.
#[derive(Debug, Clone, PartialEq)]
pub enum SpeechEvent {
    /// Normalized microphone input level in `0.0..=1.0` for drawing live waveform bars.
    Level(f32),
    /// Interim partial hypothesis while speaking or transcribing.
    Partial(String),
    /// Recording ended; on-device / OS speech engine is converting audio to text.
    Transcribing,
    /// Final recognized transcript.
    Final { text: String, engine: String },
    /// Session cancelled by the user during recording or transcription.
    Cancelled,
    /// Microphone or speech recognition permission was denied by the OS.
    PermissionDenied { kind: &'static str },
    /// Recoverable capture or recognition failure.
    Error(String),
}

/// Parameters for starting a speech recognition session.
#[derive(Debug, Clone, Default)]
pub struct SpeechRequest {
    /// Preferred BCP-47 language tag (e.g. `"ko-KR"`, `"en-US"`, `"ja-JP"`, `"zh-CN"`).
    pub language: String,
    /// Workspace file names and symbol identifiers passed as contextual vocabulary hints.
    pub hints: Vec<String>,
}

/// Handle to an active recording / transcription session.
pub trait SpeechSession: Send + Sync {
    /// Finishes audio capture and triggers transcription.
    fn stop(&self);
    /// Immediately aborts recording or in-flight transcription without emitting a final result.
    fn cancel(&self);
}

/// OS speech-recognition port.
pub trait SpeechRecognizer: Send + Sync {
    /// True when native capture + recognition can run in this process (app bundle or local Whisper CLI).
    fn available(&self) -> bool;
    /// Short identifier of the active backend (`"apple-speech"`, `"whisper-cli"`, `"unavailable"`).
    fn engine_name(&self) -> &'static str;
    /// Current OS permission state.
    fn permission(&self) -> SpeechPermission;
    /// Starts recording and streaming [`SpeechEvent`]s to `on_event`.
    fn start(&self, request: SpeechRequest, on_event: Arc<dyn Fn(SpeechEvent) + Send + Sync>) -> Result<Box<dyn SpeechSession>>;
    /// Opens the OS Privacy settings pane for Microphone / Speech Recognition.
    fn open_permission_settings(&self) -> Result<()>;
}
