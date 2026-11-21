//! Windows speech-recognition port (`MC-4`): opens Windows Privacy › Microphone settings on permission guidance clicks; live capture
//! runs through WebView2's Web Speech / WebAudio layer or local `whisper-cli` in `src-tauri/src/voice.rs`.

use std::sync::Arc;

use mighty_platform::unsupported::Unsupported;
use mighty_platform::{PlatformError, Result, SpeechEvent, SpeechPermission, SpeechRecognizer, SpeechRequest, SpeechSession};

pub struct WindowsSpeech;

pub fn speech_recognizer() -> Arc<dyn SpeechRecognizer> {
    Arc::new(WindowsSpeech)
}

impl SpeechRecognizer for WindowsSpeech {
    fn available(&self) -> bool {
        Unsupported.available()
    }

    fn engine_name(&self) -> &'static str {
        Unsupported.engine_name()
    }

    fn permission(&self) -> SpeechPermission {
        SpeechPermission::NotDetermined
    }

    fn start(&self, request: SpeechRequest, on_event: Arc<dyn Fn(SpeechEvent) + Send + Sync>) -> Result<Box<dyn SpeechSession>> {
        Unsupported.start(request, on_event)
    }

    fn open_permission_settings(&self) -> Result<()> {
        crate::open_settings_page("ms-settings:privacy-microphone").map_err(PlatformError::Refused)
    }
}
