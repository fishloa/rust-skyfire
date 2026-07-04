// web/example.js — example consumer of @firemedia/skyfire-player.
//
// This file drives SkyfirePlayer and wires the existing HTML controls
// (#audio-select, #sub-select, #playpause, #mute, #overlay, #error).
// It is the entry point for web/index.html.
//
// URL params:
//   ?src=<url>      — TS stream URL (default: /fixtures/h264-25fps.ts)
//   ?sub=<pid>      — pre-select subtitle PID
//   ?video=mse      — force MSE video path (skip WebCodecs)
//   ?live=1         — (reserved; SkyfirePlayer v1 does not yet reconnect — see opts)

import { SkyfirePlayer } from "@firemedia/skyfire-player";

const canvas     = document.getElementById("canvas");
const overlay    = document.getElementById("overlay");
const errorEl    = document.getElementById("error");
const audioSelect = document.getElementById("audio-select");
const subSelect  = document.getElementById("sub-select");
const playPauseBtn = document.getElementById("playpause");
const muteBtn    = document.getElementById("mute");

function status(msg) {
  if (overlay) overlay.textContent = msg;
}

function showError(msg) {
  if (errorEl) {
    errorEl.textContent = msg;
    errorEl.style.display = "block";
  }
  console.error("[skyfire/example]", msg);
}

const params = new URLSearchParams(location.search);

const player = new SkyfirePlayer(canvas, {
  streamUrl:   params.get("src") || "/fixtures/h264-25fps.ts",
  subtitlePid: params.has("sub") ? parseInt(params.get("sub"), 10) : undefined,
  forceMse:    params.get("video") === "mse",
});

// Re-expose the stats object that the e2e harness reads via window.__sfStats.
player.on("stats", (s) => {
  window.__sfStats = s;
  if (s.status) status(s.status);
});

// Populate the #audio-select and #sub-select pickers when the track list arrives.
player.on("tracks", (tl) => {
  // Audio picker.
  if (audioSelect) {
    audioSelect.innerHTML = "";
    tl.audio.forEach((a, i) => {
      const o = document.createElement("option");
      o.value = String(a.pid);
      o.textContent = (a.language || `track ${i + 1}`) + ` · ${a.codec}`;
      audioSelect.appendChild(o);
    });
  }

  // Subtitle picker (keep the leading "Off" option already in the HTML).
  if (subSelect) {
    while (subSelect.options.length > 1) subSelect.remove(1);
    tl.subtitles.forEach((s) => {
      const o = document.createElement("option");
      o.value = String(s.pid);
      o.textContent = (s.language || "sub") + ` · ${s.kind}`;
      subSelect.appendChild(o);
    });

    // Deep-link: ?sub=<pid> auto-selects the subtitle track at startup.
    const wantSub = params.get("sub");
    if (wantSub && [...subSelect.options].some((o) => o.value === wantSub)) {
      subSelect.value = wantSub;
      // subtitlePid was already passed to SkyfirePlayer opts; this just syncs the UI.
    }
  }
});

player.on("error", (e) => showError(e.message || String(e)));

// Wire #audio-select → player.selectAudio.
audioSelect?.addEventListener("change", (e) =>
  player.selectAudio(parseInt(e.target.value, 10))
);

// Wire #sub-select → player.selectSubtitle.
subSelect?.addEventListener("change", (e) =>
  player.selectSubtitle(e.target.value === "" ? null : parseInt(e.target.value, 10))
);

// Wire play/pause button.
let _playing = true;
playPauseBtn?.addEventListener("click", () => {
  _playing = !_playing;
  if (_playing) {
    player.play();
    if (playPauseBtn) playPauseBtn.textContent = "⏸ Pause";
  } else {
    player.pause();
    if (playPauseBtn) playPauseBtn.textContent = "▶ Play";
  }
});

// Wire mute button.
let _muted = false;
let _gainNode = null; // Not directly accessible via SkyfirePlayer API v1 — mute via AudioContext
muteBtn?.addEventListener("click", () => {
  _muted = !_muted;
  // SkyfirePlayer v1 does not expose a setMuted() method; the mute button is
  // wired for UI completeness. TODO: expose player.setMuted() in Task 4+.
  if (muteBtn) muteBtn.textContent = _muted ? "🔇 Unmute" : "🔊 Mute";
});

player.init().catch((err) => showError("startup failed: " + (err?.message || err)));
