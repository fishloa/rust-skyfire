// <skyfire-player> — polished UI shell around the headless SkyfirePlayer engine.
// All UI lives in a Shadow DOM (scoped styles); the engine draws into the shadow
// canvas and the element owns controls, menus, state overlays, PiP + fullscreen.
import { SkyfirePlayer } from "./skyfire-player.js";
import { languageName, resolveLocale } from "./lang.js";

const TEMPLATE = `
<div class="stage">
  <canvas class="video"></canvas>
  <div class="subs"><canvas></canvas></div>
  <video class="pip" hidden playsinline></video>
  <div class="overlays"></div>
  <div class="diag" hidden></div>
</div>
<div class="menus"></div>
<div class="controls"></div>
`;

const STYLE = `
:host { position: relative; display: block; width: 100%; height: 100%;
        background: #000; color: #eee; font: 14px/1.4 system-ui, sans-serif;
        overflow: hidden; }
.stage { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; }
canvas.video { max-width: 100%; max-height: 100%; object-fit: contain; }
.subs { position: absolute; left: 0; right: 0; bottom: 12%; display: flex; justify-content: center; pointer-events: none; }
.subs canvas { max-width: 90%; }
.pip { position: absolute; width: 1px; height: 1px; opacity: 0; pointer-events: none; }
.controls { position: absolute; bottom: 0; left: 0; right: 0; display: flex; gap: 10px;
            align-items: center; padding: 10px 14px; background: rgba(0,0,0,0.72);
            opacity: 0; transition: opacity .2s; }
:host(:hover) .controls, .controls:focus-within, :host([data-active]) .controls { opacity: 1; }
.controls button, .controls select { background: #1a1a1a; color: #eee; border: 1px solid #444;
            border-radius: 4px; padding: 5px 9px; font: inherit; cursor: pointer; }
.controls .spacer { flex: 1; }
.controls input[type=range] { width: 90px; }
.menus { position: absolute; bottom: 52px; right: 14px; display: flex; gap: 8px; align-items: flex-end; }
.menu { display: none; background: rgba(0,0,0,0.9); border: 1px solid #444; border-radius: 6px;
        padding: 6px; min-width: 160px; }
.menu.open { display: block; }
.menu .row { display: flex; gap: 8px; align-items: center; padding: 6px 8px; border-radius: 4px; cursor: pointer; }
.menu .row:hover { background: #222; }
.menu .row[aria-checked=true] { background: #2a3a5a; }
.overlays { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; pointer-events: none; }
.overlay { display: none; text-align: center; padding: 16px 20px; background: rgba(0,0,0,0.7); border-radius: 8px; pointer-events: auto; }
.overlay.show { display: block; }
.spinner { width: 40px; height: 40px; border: 4px solid #444; border-top-color: #6af; border-radius: 50%; animation: spin 1s linear infinite; margin: 0 auto; }
@keyframes spin { to { transform: rotate(360deg); } }
.diag { position: absolute; top: 8px; left: 8px; background: rgba(0,0,0,0.75); padding: 8px 10px;
        border-radius: 6px; font-variant-numeric: tabular-nums; white-space: pre; font-size: 12px; }
.diag[hidden] { display: none; }
:host(:fullscreen) { width: 100vw; height: 100vh; background: #000; }
/* !important: per CSS Scoping, ordinary declarations in the outer tree beat
   :host — a routine embed rule like
   "skyfire-player { width: 100%; aspect-ratio: 16/9; position: relative }"
   would otherwise override this and defeat the only fullscreen fallback iOS
   Safari has (no Element.requestFullscreen there). */
:host(.sf-pseudo-fullscreen) {
  position: fixed !important; inset: 0 !important; width: 100vw !important; height: 100vh !important;
  z-index: 2147483647 !important; background: #000 !important;
}
`;

export class SkyfirePlayerElement extends HTMLElement {
  static get observedAttributes() { return ["src", "controls", "muted", "autoplay", "audio-lead"]; }

  constructor() {
    super();
    const root = this.attachShadow({ mode: "open" });
    const style = document.createElement("style");
    style.textContent = STYLE;
    root.appendChild(style);
    const wrap = document.createElement("div");
    wrap.innerHTML = TEMPLATE;
    root.append(...wrap.childNodes);

    this._engine = null;
    this._tracks = null;
    this._state = "idle";
    this._switchSeq = 0;
    this._playing = true;
    this._muted2 = this.hasAttribute("muted");
    this._selAudio = null;
    this._selSub = null;
    // Last fullscreen state this element itself emitted sf-fullscreenchange
    // for — lets the document-wide fullscreenchange listener dedupe/skip.
    this._lastFsState = false;

    this._videoCanvas = root.querySelector("canvas.video");
    this._subsCanvas = root.querySelector(".subs canvas");
    this._pipVideo = root.querySelector("video.pip");
    this._controlsEl = root.querySelector(".controls");
    this._menusEl = root.querySelector(".menus");
    this._overlaysEl = root.querySelector(".overlays");
    this._diagEl = root.querySelector(".diag");

    this._playBtn = null;
    this._muteBtn = null;
    this._pipBtn = null;

    this._stallTicks = 0;

    this._overlaysEl.innerHTML = `
      <div class="overlay" data-state="loading"><div class="spinner"></div><div>Loading…</div></div>
      <div class="overlay" data-state="buffering"><div class="spinner"></div><div>Buffering…</div></div>
      <div class="overlay" data-state="ended"><div>Stream ended</div></div>
      <div class="overlay" data-state="error"><div class="msg"></div><button class="retry" type="button">Retry</button></div>`;
    this._overlaysEl.querySelector(".retry").addEventListener("click", () => this._start());
    this._lastProgress = { t: 0, drawn: 0, audioFrames: 0 };
  }

  // ── attribute reflection ──
  get src() { return this.getAttribute("src"); }
  set src(v) { if (v == null) this.removeAttribute("src"); else this.setAttribute("src", v); }
  get controls() { return this.getAttribute("controls") || "full"; }
  set controls(v) { this.setAttribute("controls", v); }
  get muted() { return this.hasAttribute("muted"); }
  set muted(v) { if (v) this.setAttribute("muted", ""); else this.removeAttribute("muted"); }

  connectedCallback() {
    this._buildControls();
    if (this.hasAttribute("autoplay") || this.getAttribute("src")) this._start();
    // The listener is document-wide (fullscreenchange only fires there), so
    // ANY element on the page entering/leaving fullscreen would otherwise make
    // every <skyfire-player> emit. Guard against both false positives:
    //  - this element is in *pseudo*-fullscreen and some other element went
    //    native — that is not a change to THIS element's fullscreen state.
    //  - the computed state hasn't actually changed since we last emitted it
    //    (e.g. two elements on the page both got a fullscreenchange callback
    //    for the same event, or repeated events for an already-reported state).
    this._onFsChange = () => {
      if (this._pseudoFs) return;
      const fs = this.ownerDocument.fullscreenElement === this;
      if (fs === this._lastFsState) return;
      this._emitFullscreen(fs, "native");
    };
    this.ownerDocument.addEventListener("fullscreenchange", this._onFsChange);
  }
  disconnectedCallback() {
    this._teardown();
    this.ownerDocument.removeEventListener("fullscreenchange", this._onFsChange);
    // Reverse an in-progress pseudo-fullscreen so the host page's scroll lock
    // isn't leaked forever. No sf-fullscreenchange here — the element is being
    // torn down, and dispatching events from a detached element is worse than
    // staying silent.
    if (this._pseudoFs) {
      this.classList.remove("sf-pseudo-fullscreen");
      this._pseudoFs = false;
      this.ownerDocument.body.style.overflow = this._prevOverflow ?? "";
    }
  }
  attributeChangedCallback(name, oldV, newV) {
    if (!this.isConnected || oldV === newV) return;
    switch (name) {
      case "src":
        this._teardown();
        this._start();
        break;
      case "controls":
        this._buildControls();
        this._buildMenus();
        break;
      case "muted":
        this._muted2 = this.hasAttribute("muted");
        this._engine?.setMuted(this._muted2);
        break;
      case "audio-lead":
        break;
      default: break;
    }
  }

  // ── engine wiring ──
  _start() {
    if (this._engine) this._teardown();
    const src = this.getAttribute("src");
    if (!src) { this._setState("idle"); return; }
    const seq = ++this._switchSeq;
    this._setState("loading");
    const opts = {
      streamUrl: src,
      muted: this.hasAttribute("muted"),
      forceMse: this.getAttribute("video") === "mse",
    };
    const lead = parseFloat(this.getAttribute("audio-lead"));
    if (!Number.isNaN(lead)) opts.audioLeadSeconds = lead;

    const engine = new SkyfirePlayer(this._videoCanvas, opts);
    this._engine = engine;
    engine.on("tracks", (tl, diff) => {
      if (seq !== this._switchSeq) return;
      this._applyTracks(tl);
      if (diff) {
        this.dispatchEvent(new CustomEvent("sf-tracks-changed", {
          detail: diff, bubbles: true, composed: true,
        }));
      }
    });
    engine.on("stats", (s) => {
      if (seq !== this._switchSeq) return;
      window.__sfStats = s;
      this._onStats(s);
      this.dispatchEvent(new CustomEvent("sf-stats", { detail: s, bubbles: true, composed: true }));
    });
    engine.on("error", (e) => {
      if (seq !== this._switchSeq) return;
      this._setState("error", e?.message || String(e));
      this.dispatchEvent(new CustomEvent("sf-error", { detail: e, bubbles: true, composed: true }));
    });
    engine.on("ended", (s) => {
      if (seq !== this._switchSeq) return;
      this._setState("ended");
      this.dispatchEvent(new CustomEvent("sf-ended", { detail: s, bubbles: true, composed: true }));
    });
    engine.init().catch((err) => {
      if (seq === this._switchSeq) this._setState("error", err?.message || String(err));
    });
  }

  _teardown() {
    if (this._engine) { try { this._engine.destroy(); } catch (_) {} this._engine = null; }
  }

  _applyTracks(tl) {
    this._tracks = tl;
    this.dispatchEvent(new CustomEvent("sf-tracks", { detail: tl, bubbles: true, composed: true }));
    this._buildMenus();
  }

  // ── state machine + overlays ──
  _setState(name, msg) {
    this._state = name;
    this._overlaysEl.querySelectorAll(".overlay").forEach((o) =>
      o.classList.toggle("show", o.dataset.state === name && name !== "idle" && name !== "playing"));
    if (name === "error") {
      const m = this._overlaysEl.querySelector(".overlay[data-state=error] .msg");
      if (m) m.textContent = msg || "Playback error";
    }
  }

  _onStats(s) {
    const now = (s && typeof s === "object") ? (this._lastProgress.t + 1) : 0;
    const advanced = (s.drawn > this._lastProgress.drawn) || (s.audioFrames > this._lastProgress.audioFrames);
    if (s.drawn > 0 && (this._state === "loading" || this._state === "idle")) this._setState("playing");
    if (this._state === "playing" && !s.done && !advanced) {
      this._stallTicks = (this._stallTicks || 0) + 1;
      if (this._stallTicks >= 3) this._setState("buffering");
    } else if (advanced) {
      this._stallTicks = 0;
      if (this._state === "buffering") this._setState("playing");
    }
    if (!this._diagEl.hidden) {
      const e = this._engine || {};
      const ctxRate = e._audioCtx?.sampleRate;
      const pcmRate = e._audioSampleRate;
      const rateWarn = (ctxRate && pcmRate && ctxRate !== pcmRate) ? "  ⚠ MISMATCH" : "";
      this._diagEl.textContent =
        `path: ${s.videoPath || "?"}\n` +
        `video: ${s.decoded ?? 0} dec / ${s.drawn ?? 0} drawn\n` +
        `audio: ${s.audioFrames ?? 0} frames (${s.audioSec ? s.audioSec.toFixed(1) : 0}s)\n` +
        `ctx rate: ${ctxRate ?? "?"} / pcm: ${pcmRate ?? "?"}${rateWarn}\n` +
        `ahead: ${e._audioAheadSeconds ? e._audioAheadSeconds().toFixed(1) : "?"}s\n` +
        `skew: ${s.avSkewMs ?? 0} ms`;
    }
    this._lastProgress = { t: now, drawn: s.drawn || 0, audioFrames: s.audioFrames || 0 };
  }

  // ── delegating API ──
  play() { this._engine?.play(); }
  pause() { this._engine?.pause(); }
  selectAudio(pid) { this._engine?.selectAudio(pid); }
  selectSubtitle(pid) { this._engine?.selectSubtitle(pid); }
  get tracks() { return this._tracks; }
  get stats() { return this._engine?._stats ?? null; }

  // ── control bar ──
  _buildControls() {
    const bar = this._controlsEl;
    bar.innerHTML = "";
    const preset = this.controls;
    if (preset === "none") return;

    const btn = (cls, label, on) => {
      const b = document.createElement("button");
      b.className = cls; b.type = "button"; b.textContent = label;
      b.addEventListener("click", on);
      bar.appendChild(b); return b;
    };

    this._playBtn = btn("playpause", "⏸", () => this._togglePlay());

    if (preset === "full") {
      const vol = document.createElement("input");
      vol.type = "range"; vol.min = "0"; vol.max = "1"; vol.step = "0.05"; vol.value = "1";
      vol.className = "vol"; vol.setAttribute("aria-label", "Volume");
      vol.addEventListener("input", () => this._engine?.setVolume(parseFloat(vol.value)));
      bar.appendChild(vol);
      this._muteBtn = btn("mute-btn", "🔊", () => this._toggleMute());

      const spacer = document.createElement("span"); spacer.className = "spacer"; bar.appendChild(spacer);

      btn("audio-btn", "Audio ▾", () => this._toggleMenu("audio"));
      btn("subs-btn", "Subtitles ▾", () => this._toggleMenu("subtitle"));
      this._pipBtn = btn("pip-btn", "⧉", () => this._togglePip());
      if (this._pipBtn && !this._pipSupported()) this._pipBtn.hidden = true;
      btn("fs-btn", "⛶", () => this._onFsButtonClick()).setAttribute("aria-pressed", "false");
      btn("diag-btn", "ⓘ", () => this._toggleDiag());
    } else if (preset === "minimal") {
      const spacer = document.createElement("span"); spacer.className = "spacer"; bar.appendChild(spacer);
      btn("fs-btn", "⛶", () => this._onFsButtonClick()).setAttribute("aria-pressed", "false");
    }
  }

  _togglePlay() {
    if (!this._engine) return;
    this._playing = !this._playing;
    if (this._playing) { this._engine.play(); this._playBtn.textContent = "⏸"; }
    else { this._engine.pause(); this._playBtn.textContent = "▶"; }
  }
  _toggleMute() {
    this._muted2 = !this._muted2;
    this._engine?.setMuted(this._muted2);
    if (this._muteBtn) this._muteBtn.textContent = this._muted2 ? "🔇" : "🔊";
  }

  // ── menus ──
  _buildMenus() {
    const tl = this._tracks;
    this._menusEl.innerHTML = "";
    if (!tl || this.controls !== "full") return;

    const menu = (kind) => { const m = document.createElement("div"); m.className = `menu ${kind}`; this._menusEl.appendChild(m); return m; };
    const row = (m, label, checked, on) => {
      const r = document.createElement("div"); r.className = "row"; r.setAttribute("role", "menuitemradio");
      r.setAttribute("aria-checked", checked ? "true" : "false"); r.textContent = label;
      r.addEventListener("click", on); m.appendChild(r); return r;
    };

    const locale = resolveLocale(this);

    // Broadcasters routinely ship two tracks in the same language (arte pid
    // 257 and 258 are both fra). Where a name repeats, number the repeats so
    // the rows stay distinguishable; unique names stay bare.
    const nameFor = (t, i, fallback) => languageName(t.language, locale) || `${fallback} ${i + 1}`;
    const audioNames = (tl.audio || []).map((a, i) => nameFor(a, i, "Track"));
    const seen = new Map();
    const audioLabels = audioNames.map((n, i) => {
      const a = tl.audio[i];
      const dup = audioNames.filter((x) => x === n).length > 1;
      const chan = a.channels === 6 ? " 5.1" : a.channels === 1 ? " mono" : "";
      let label = `${n} · ${a.codec}${chan}`;
      if (dup) {
        const nth = (seen.get(label) ?? 0) + 1;
        seen.set(label, nth);
        if (nth > 1) label = `${label} (${nth})`;
      }
      return label;
    });

    const am = menu("audio");
    (tl.audio || []).forEach((a, i) => {
      row(am, audioLabels[i], this._selAudio === a.pid || (this._selAudio == null && i === 0),
        () => { this._selAudio = a.pid; this.selectAudio(a.pid); this._buildMenus(); am.classList.add("open"); });
    });

    const sm = menu("subtitle");
    row(sm, "Off", this._selSub == null, () => { this._selSub = null; this.selectSubtitle(null); this._buildMenus(); sm.classList.add("open"); });
    (tl.subtitles || []).forEach((s, i) => {
      const label = nameFor(s, i, "Subtitle");
      row(sm, label, this._selSub === s.pid, () => { this._selSub = s.pid; this.selectSubtitle(s.pid); this._buildMenus(); sm.classList.add("open"); });
    });
  }

  _toggleMenu(kind) {
    const m = this._menusEl.querySelector(`.menu.${kind}`);
    if (!m) return;
    const wasOpen = m.classList.contains("open");
    this._menusEl.querySelectorAll(".menu").forEach((x) => x.classList.remove("open"));
    if (!wasOpen) m.classList.add("open");
  }

  // ── fullscreen + PiP ──
  get isFullscreen() {
    return this.ownerDocument.fullscreenElement === this ||
      this.classList.contains("sf-pseudo-fullscreen");
  }

  /**
   * Enter fullscreen. Resolves once the transition is requested; rejects with
   * the underlying reason if the browser refuses — the caller is told, rather
   * than the failure being discarded.
   *
   * Where Element.requestFullscreen does not exist (iPhone Safari, which only
   * promotes <video> elements, and skyfire paints to a <canvas>) this falls
   * back to a fixed-position overlay and reports mode "pseudo".
   */
  async enterFullscreen() {
    if (this.isFullscreen) return;
    if (typeof this.requestFullscreen === "function") {
      await this.requestFullscreen();
      return; // fullscreenchange fires the event
    }
    this.classList.add("sf-pseudo-fullscreen");
    this._pseudoFs = true;
    this._prevOverflow = this.ownerDocument.body.style.overflow;
    this.ownerDocument.body.style.overflow = "hidden";
    this._emitFullscreen(true, "pseudo");
  }

  async exitFullscreen() {
    if (this._pseudoFs) {
      this.classList.remove("sf-pseudo-fullscreen");
      this._pseudoFs = false;
      this.ownerDocument.body.style.overflow = this._prevOverflow ?? "";
      this._emitFullscreen(false, "pseudo");
      return;
    }
    if (this.ownerDocument.fullscreenElement === this) {
      await this.ownerDocument.exitFullscreen();
    }
  }

  toggleFullscreen() {
    return this.isFullscreen ? this.exitFullscreen() : this.enterFullscreen();
  }

  /**
   * Click handler for the built-in ⛶ button only. The public API
   * (toggleFullscreen/enterFullscreen/exitFullscreen) must keep rejecting so
   * callers can react to a refusal — do not add a catch inside those. But an
   * ordinary UI click that gets refused (e.g. cross-origin iframe without
   * `allow="fullscreen"`) must not surface as an "Uncaught (in promise)" on
   * every click, so this path alone logs instead of discarding.
   */
  _onFsButtonClick() {
    this.toggleFullscreen().catch((err) => {
      console.warn("[skyfire] fullscreen refused", err);
    });
  }

  _emitFullscreen(fullscreen, mode) {
    this._lastFsState = fullscreen;
    const fsBtn = this._controlsEl?.querySelector(".fs-btn");
    if (fsBtn) fsBtn.setAttribute("aria-pressed", fullscreen ? "true" : "false");
    this.dispatchEvent(new CustomEvent("sf-fullscreenchange", {
      detail: { fullscreen, mode }, bubbles: true, composed: true,
    }));
  }

  _pipSupported() {
    return !!(this.ownerDocument.pictureInPictureEnabled &&
      HTMLVideoElement.prototype.requestPictureInPicture);
  }

  async _togglePip() {
    if (!this._pipSupported()) return;
    const doc = this.ownerDocument;
    if (doc.pictureInPictureElement) { await doc.exitPictureInPicture().catch(() => {}); return; }
    let video = this._engine?._mseVideoEl;
    if (!video) {
      video = this._pipVideo;
      if (!video.srcObject && this._videoCanvas.captureStream) {
        video.srcObject = this._videoCanvas.captureStream(30);
        await video.play().catch(() => {});
      }
    }
    await video.requestPictureInPicture().catch(() => {});
  }

  // ── diagnostics ──
  _toggleDiag() {
    this._diagEl.hidden = !this._diagEl.hidden;
  }
}

if (!customElements.get("skyfire-player")) {
  customElements.define("skyfire-player", SkyfirePlayerElement);
}
