package dev.mightycode.speech

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import androidx.core.content.ContextCompat
import expo.modules.interfaces.permissions.PermissionsStatus
import expo.modules.kotlin.Promise
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition

/**
 * Native Android speech-to-text module for Mighty Code mobile remote (`MC-4`).
 * Drives `android.speech.SpeechRecognizer` on the main looper with live RMS levels,
 * partial hypotheses, stop-to-transcribe, and immediate cancellation.
 */
class MightySpeechModule : Module() {
  private val mainHandler = Handler(Looper.getMainLooper())
  private var recognizer: SpeechRecognizer? = null
  private var cancelled = false

  private fun rmsToLevel(rmsdB: Float): Double {
    if (!rmsdB.isFinite()) return 0.14
    val normalized = ((rmsdB + 2.0f) / 12.0f).coerceIn(0.14f, 1.0f)
    return normalized.toDouble()
  }

  private fun destroyRecognizer() {
    recognizer?.let { rec ->
      try {
        rec.destroy()
      } catch (_: Throwable) {
      }
    }
    recognizer = null
  }

  override fun definition() = ModuleDefinition {
    Name("MightySpeech")

    Events("onSpeechEvent")

    Function("isAvailable") {
      val context = appContext.reactContext ?: return@Function false
      SpeechRecognizer.isRecognitionAvailable(context)
    }

    AsyncFunction("getPermissionStatus") { promise: Promise ->
      val permissions = appContext.permissions
      if (permissions != null) {
        permissions.getPermissions(
          { result ->
            val status = result[Manifest.permission.RECORD_AUDIO]?.status ?: PermissionsStatus.UNDETERMINED
            promise.resolve(status.status)
          },
          Manifest.permission.RECORD_AUDIO
        )
        return@AsyncFunction
      }
      val context = appContext.reactContext
      if (context == null) {
        promise.resolve("denied")
        return@AsyncFunction
      }
      val granted = ContextCompat.checkSelfPermission(
        context,
        Manifest.permission.RECORD_AUDIO
      ) == PackageManager.PERMISSION_GRANTED
      promise.resolve(if (granted) "granted" else "undetermined")
    }

    AsyncFunction("requestPermission") { promise: Promise ->
      val permissions = appContext.permissions
      if (permissions == null) {
        promise.resolve("denied")
        return@AsyncFunction
      }
      permissions.askForPermissions(
        { result ->
          val status = result[Manifest.permission.RECORD_AUDIO]?.status ?: PermissionsStatus.DENIED
          promise.resolve(status.status)
        },
        Manifest.permission.RECORD_AUDIO
      )
    }

    AsyncFunction("start") { locale: String, _hints: List<String>, promise: Promise ->
      val context = appContext.reactContext
      if (context == null || !SpeechRecognizer.isRecognitionAvailable(context)) {
        promise.reject("ERR_SPEECH_UNAVAILABLE", "Speech recognition is unavailable", null)
        return@AsyncFunction
      }
      val hasMic = ContextCompat.checkSelfPermission(
        context,
        Manifest.permission.RECORD_AUDIO
      ) == PackageManager.PERMISSION_GRANTED
      if (!hasMic) {
        sendEvent("onSpeechEvent", mapOf("type" to "permissionDenied"))
        promise.resolve(false)
        return@AsyncFunction
      }

      mainHandler.post {
        destroyRecognizer()
        cancelled = false
        val rec = SpeechRecognizer.createSpeechRecognizer(context)
        recognizer = rec
        rec.setRecognitionListener(object : RecognitionListener {
          override fun onReadyForSpeech(params: Bundle?) {
            if (!cancelled) sendEvent("onSpeechEvent", mapOf("type" to "recording"))
          }

          override fun onBeginningOfSpeech() {}

          override fun onRmsChanged(rmsdB: Float) {
            if (!cancelled) {
              sendEvent("onSpeechEvent", mapOf("type" to "level", "level" to rmsToLevel(rmsdB)))
            }
          }

          override fun onBufferReceived(buffer: ByteArray?) {}

          override fun onEndOfSpeech() {
            if (!cancelled) sendEvent("onSpeechEvent", mapOf("type" to "transcribing"))
          }

          override fun onError(error: Int) {
            destroyRecognizer()
            if (cancelled) return
            if (error == SpeechRecognizer.ERROR_NO_MATCH || error == SpeechRecognizer.ERROR_SPEECH_TIMEOUT) {
              sendEvent("onSpeechEvent", mapOf("type" to "result", "text" to ""))
            } else if (error == SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS) {
              sendEvent("onSpeechEvent", mapOf("type" to "permissionDenied"))
            } else {
              sendEvent("onSpeechEvent", mapOf("type" to "error", "message" to "speech_error_$error"))
            }
          }

          override fun onResults(results: Bundle?) {
            destroyRecognizer()
            if (cancelled) return
            val matches = results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
            val best = matches?.firstOrNull() ?: ""
            sendEvent("onSpeechEvent", mapOf("type" to "result", "text" to best))
          }

          override fun onPartialResults(partialResults: Bundle?) {
            if (cancelled) return
            val matches = partialResults?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)
            val best = matches?.firstOrNull() ?: ""
            if (best.isNotEmpty()) {
              sendEvent("onSpeechEvent", mapOf("type" to "partial", "text" to best))
            }
          }

          override fun onEvent(eventType: Int, params: Bundle?) {}
        })

        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
          putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
          putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
          if (locale.isNotBlank()) {
            putExtra(RecognizerIntent.EXTRA_LANGUAGE, locale)
          }
        }
        rec.startListening(intent)
        promise.resolve(true)
      }
    }

    AsyncFunction("stop") { promise: Promise ->
      mainHandler.post {
        if (!cancelled) {
          sendEvent("onSpeechEvent", mapOf("type" to "transcribing"))
          recognizer?.stopListening()
        }
        promise.resolve(null)
      }
    }

    AsyncFunction("cancel") { promise: Promise ->
      mainHandler.post {
        cancelled = true
        try {
          recognizer?.cancel()
        } catch (_: Throwable) {
        }
        destroyRecognizer()
        sendEvent("onSpeechEvent", mapOf("type" to "cancelled"))
        promise.resolve(null)
      }
    }

    OnDestroy {
      mainHandler.post {
        cancelled = true
        destroyRecognizer()
      }
    }
  }
}
