// video-probe.js — Native <video>/HLS interlaced H.264 + E-AC-3 diagnostic.
// Counterpart to ios-probe.js (WebCodecs path, which failed on all browsers).
// Tests iOS AVFoundation/VideoToolbox hardware decode of 1080i interlaced H.264
// and E-AC-3 audio via the native HLS pipeline — no WASM required.
// Fixture: fixtures/gulli.m3u8  (single-segment VOD wrapping gulli-15s.ts)

// ── logging helpers ─────────────────────────────────────────────────────────

const logEl = document.getElementById("log");

function log(text, cls) {
  const d = document.createElement("div");
  d.className = "entry " + (cls || "info");
  d.textContent = text;
  logEl.appendChild(d);
  console.log("[video-probe]", text);
  return d;
}

function stepHead(n, label) {
  const d = document.createElement("div");
  d.className = "entry info step-head";
  d.textContent = "── Step " + n + ": " + label + " ──";
  logEl.appendChild(d);
  console.log("[video-probe] step " + n + ": " + label);
}

// ── main probe ───────────────────────────────────────────────────────────────

function runProbe() {

  // Step 1 — Environment
  stepHead(1, "Environment");
  log("UA: " + navigator.userAgent);
  log("hardwareConcurrency: " + (navigator.hardwareConcurrency != null
    ? navigator.hardwareConcurrency
    : "unknown"));

  var hlsCap = document.createElement("video")
    .canPlayType("application/vnd.apple.mpegurl");
  log(
    "canPlayType(application/vnd.apple.mpegurl): \""
    + (hlsCap || "")
    + "\""
    + (hlsCap ? "  ✓ native HLS supported" : "  ✗ no native HLS (not Safari)"),
    hlsCap ? "ok" : "warn"
  );

  // Step 2 — Native <video> playback attempt
  stepHead(2, "Native <video> playback");

  var video = document.getElementById("probe-video");
  var timecodeEl = document.getElementById("timecode");

  // Track whether we have already fired the final result.
  var resultFired = false;

  function fireResult() {
    if (resultFired) return;
    resultFired = true;

    stepHead(3, "Result");

    var rs = video.readyState;
    var ns = video.networkState;
    var rsLabel = ["HAVE_NOTHING", "HAVE_METADATA", "HAVE_CURRENT_DATA",
      "HAVE_FUTURE_DATA", "HAVE_ENOUGH_DATA"][rs] || rs;
    var nsLabel = ["NETWORK_EMPTY", "NETWORK_IDLE", "NETWORK_LOADING",
      "NETWORK_NO_SOURCE"][ns] || ns;

    if (video.videoWidth > 0 && video.currentTime > 0 && !video.error) {
      log(
        "NATIVE HLS DECODE: WORKS ✅\n"
        + video.videoWidth + "x" + video.videoHeight
        + ", played " + video.currentTime.toFixed(2) + "s\n"
        + "readyState=" + rsLabel + "  networkState=" + nsLabel,
        "result-ok"
      );
    } else {
      var errDetail = "";
      if (video.error) {
        errDetail = "\nerror.code=" + video.error.code
          + "  error.message=" + (video.error.message || "(none)");
      }
      log(
        "NATIVE HLS DECODE: FAILED ❌\n"
        + "videoWidth=" + video.videoWidth
        + "  currentTime=" + video.currentTime.toFixed(2) + "s"
        + errDetail + "\n"
        + "readyState=" + rsLabel + "  networkState=" + nsLabel,
        "result-fail"
      );
    }

    log("Tap the button above to unmute and confirm E-AC-3 audio.", "warn");
  }

  // Wire events before setting src.
  video.addEventListener("loadedmetadata", function () {
    log(
      "loadedmetadata: " + video.videoWidth + "x" + video.videoHeight
      + "  duration=" + (isFinite(video.duration)
        ? video.duration.toFixed(2) + "s"
        : video.duration),
      "ok"
    );
  });

  video.addEventListener("canplay", function () {
    log("canplay", "ok");
  });

  video.addEventListener("playing", function () {
    log("playing", "ok");
  });

  video.addEventListener("stalled", function () {
    log("stalled", "warn");
  });

  video.addEventListener("waiting", function () {
    log("waiting", "warn");
  });

  video.addEventListener("error", function () {
    var err = video.error;
    log(
      "error event: code=" + (err ? err.code : "?")
      + "  message=" + (err ? (err.message || "(none)") : "?"),
      "error"
    );
  });

  // timeupdate — overwrite the timecode line rather than spamming log.
  video.addEventListener("timeupdate", function () {
    timecodeEl.textContent = "currentTime: " + video.currentTime.toFixed(2) + "s";
  });

  // Set source and play.
  log("Setting src → fixtures/gulli.m3u8");
  video.src = "fixtures/gulli.m3u8";

  video.play().catch(function (e) {
    log("play() rejected: " + (e && e.message ? e.message : String(e)), "warn");
    // Autoplay blocked by browser policy is normal; the controls allow manual play.
  });

  // After 6 seconds, evaluate and emit the big result line.
  setTimeout(function () {
    fireResult();
  }, 6000);

  // Also fire early if the video ends before the timer.
  video.addEventListener("ended", function () {
    log("ended", "ok");
    fireResult();
  });

  // ── Audio button ───────────────────────────────────────────────────────────
  var audioBtn = document.getElementById("audio-btn");
  audioBtn.addEventListener("click", function () {
    video.muted = false;
    video.play().catch(function (e) {
      log("unmuted play() rejected: " + (e && e.message ? e.message : String(e)), "warn");
    });
    log("Audio unmuted — listen for E-AC-3 decode (should hear dialogue/music).", "info");
    audioBtn.textContent = "▶ Playing with sound…";
    audioBtn.disabled = true;
  });
}

// Kick off after DOM is ready (script is at bottom of body so it's fine, but
// guard anyway for any future move above the fold).
if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", runProbe);
} else {
  runProbe();
}
