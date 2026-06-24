/* @ts-self-types="./skyfire_wasm.d.ts" */

/**
 * Result of probing MPEG-TS bytes for the channel map (PAT+PMT).
 */
export class ProbeResult {
    static __wrap(ptr) {
        const obj = Object.create(ProbeResult.prototype);
        obj.__wbg_ptr = ptr;
        ProbeResultFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        ProbeResultFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_proberesult_free(ptr, 0);
    }
    /**
     * Video codec identifier: `"H264"` or `"H265"`.
     * @returns {string}
     */
    get video_codec() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.__wbg_get_proberesult_video_codec(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * PID of the video elementary stream.
     * @returns {number}
     */
    get video_pid() {
        const ret = wasm.__wbg_get_proberesult_video_pid(this.__wbg_ptr);
        return ret;
    }
    /**
     * Audio codec strings, parallel to `audio_pids`.
     * @returns {string[]}
     */
    get audio_codecs() {
        const ret = wasm.proberesult_audio_codecs(this.__wbg_ptr);
        var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * PIDs of audio elementary streams.
     * @returns {Uint16Array}
     */
    get audio_pids() {
        const ret = wasm.proberesult_audio_pids(this.__wbg_ptr);
        var v1 = getArrayU16FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 2, 2);
        return v1;
    }
    /**
     * Video codec identifier: `"H264"` or `"H265"`.
     * @param {string} arg0
     */
    set video_codec(arg0) {
        const ptr0 = passStringToWasm0(arg0, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.__wbg_set_proberesult_video_codec(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * PID of the video elementary stream.
     * @param {number} arg0
     */
    set video_pid(arg0) {
        wasm.__wbg_set_proberesult_video_pid(this.__wbg_ptr, arg0);
    }
}
if (Symbol.dispose) ProbeResult.prototype[Symbol.dispose] = ProbeResult.prototype.free;

/**
 * Streaming WASM bridge between the browser and the Skyfire demux engine.
 *
 * Unlike [`WasmEngine`] (which requires probe→init→feed→finalize), this
 * struct is designed for real-time streaming:
 *
 * 1. Construct with `SkyfireBridge::new()`.
 * 2. Call `feed(chunk)` repeatedly as TS data arrives over `fetch()`.
 * 3. Poll `track_list()` until it becomes `Some` (PAT+PMT have been parsed).
 * 4. Call `take_video_aus()` each tick to drain pending video access units.
 * 5. Use `pcr_pts()` for the A/V sync clock.
 */
export class SkyfireBridge {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        SkyfireBridgeFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_skyfirebridge_free(ptr, 0);
    }
    /**
     * Push a raw TS chunk into the bridge.
     *
     * Demuxes PAT/PMT on the fly and accumulates video AUs.
     * @param {Uint8Array} bytes
     */
    feed(bytes) {
        const ptr0 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.skyfirebridge_feed(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * Signal end-of-stream: flush any partial PES packets held in the
     * PES assemblers, then run the same access-unit processing as `feed()`.
     *
     * After calling `flush()`, a subsequent `take_video_aus()` /
     * `take_audio_pcm()` will return any tail access units that were
     * held back because the final PES end had not yet been signalled by
     * a downstream PUSI packet.  Safe to call once at stream end;
     * idempotent — calling it more than once does nothing harmful.
     */
    flush() {
        wasm.skyfirebridge_flush(this.__wbg_ptr);
    }
    /**
     * Create a new, empty bridge.
     */
    constructor() {
        const ret = wasm.skyfirebridge_new();
        this.__wbg_ptr = ret;
        SkyfireBridgeFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Latest PCR-derived clock value in 90 kHz ticks.
     *
     * The `EsDemux` / `SiDemux` layer does not separately surface PCR values;
     * we derive this from the most recently seen video or selected-audio PTS,
     * which is within one PCR interval (~40 ms for DVB) of the true PCR.
     * A future issue can replace this with raw PCR extraction if sub-millisecond
     * accuracy is required (verified 2026-06-22).
     * @returns {bigint | undefined}
     */
    pcr_pts() {
        const ret = wasm.skyfirebridge_pcr_pts(this.__wbg_ptr);
        return ret[0] === 0 ? undefined : ret[1];
    }
    /**
     * Select which audio PID to route and decode.
     *
     * If the PID changes, the AC-3/E-AC-3 and MPEG audio decoder states are
     * reset so the new stream decodes cleanly (PTS continuity is handled in
     * issue #33).
     * @param {number} pid
     */
    select_audio(pid) {
        wasm.skyfirebridge_select_audio(this.__wbg_ptr, pid);
    }
    /**
     * Select a subtitle PID, or `None` to disable subtitles.
     *
     * Calling this clears any buffered subtitle cues from the previously
     * selected PID (or disables subtitle output when `pid` is `None`).
     * @param {number | null} [pid]
     */
    select_subtitle(pid) {
        wasm.skyfirebridge_select_subtitle(this.__wbg_ptr, isLikeNone(pid) ? 0xFFFFFF : pid);
    }
    /**
     * Set the play/pause state (stored; gates nothing critical yet).
     * @param {boolean} playing
     */
    set_playing(playing) {
        wasm.skyfirebridge_set_playing(this.__wbg_ptr, playing);
    }
    /**
     * Drain all decoded PCM chunks produced since the last call.
     *
     * Each chunk corresponds to one audio access unit decoded from the
     * selected audio PID.  Samples are interleaved f32 (WebAudio-ready).
     * @returns {WasmPcmChunk[]}
     */
    take_audio_pcm() {
        const ret = wasm.skyfirebridge_take_audio_pcm(this.__wbg_ptr);
        var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * Drain all composited subtitle cues since the last call.
     *
     * Each cue corresponds to one DVB subtitle display-set from the selected
     * subtitle PID.  Each cue contains RGBA region bitmaps ready for the
     * JS overlay (no further parsing needed).
     *
     * Returns an empty `Vec` when no subtitle PID is selected
     * (`select_subtitle(None)`) or when the selected PID carries no subtitle
     * PES packets in the fed data (e.g. a fixture without subtitle tracks).
     * @returns {WasmSubtitleCue[]}
     */
    take_subtitle_cues() {
        const ret = wasm.skyfirebridge_take_subtitle_cues(this.__wbg_ptr);
        var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * Drain all completed video access units since the last call.
     *
     * Returns Annex-B bytes with PTS/DTS and a keyframe flag.
     * @returns {WasmVideoAu[]}
     */
    take_video_aus() {
        const ret = wasm.skyfirebridge_take_video_aus(this.__wbg_ptr);
        var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * Returns the track list once a PMT has been parsed, or `None`.
     * @returns {WasmTrackList | undefined}
     */
    track_list() {
        const ret = wasm.skyfirebridge_track_list(this.__wbg_ptr);
        return ret === 0 ? undefined : WasmTrackList.__wrap(ret);
    }
    /**
     * WebCodecs codec string (e.g. `"avc1.640028"`) once SPS has been seen.
     *
     * Returns `None` until sufficient video AUs have been fed to extract an SPS.
     * @returns {string | undefined}
     */
    video_codec() {
        const ret = wasm.skyfirebridge_video_codec(this.__wbg_ptr);
        let v1;
        if (ret[0] !== 0) {
            v1 = getStringFromWasm0(ret[0], ret[1]).slice();
            wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        }
        return v1;
    }
}
if (Symbol.dispose) SkyfireBridge.prototype[Symbol.dispose] = SkyfireBridge.prototype.free;

/**
 * One audio elementary stream.
 */
export class WasmAudioTrack {
    static __wrap(ptr) {
        const obj = Object.create(WasmAudioTrack.prototype);
        obj.__wbg_ptr = ptr;
        WasmAudioTrackFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    static __unwrap(jsValue) {
        if (!(jsValue instanceof WasmAudioTrack)) {
            return 0;
        }
        return jsValue.__destroy_into_raw();
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmAudioTrackFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmaudiotrack_free(ptr, 0);
    }
    /**
     * `"AC3"`, `"EAC3"`, or `"MP2"`.
     * @returns {string}
     */
    get codec() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.__wbg_get_wasmaudiotrack_codec(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * ISO 639-2 language (3 chars), or `None`.
     * @returns {string | undefined}
     */
    get language() {
        const ret = wasm.__wbg_get_wasmaudiotrack_language(this.__wbg_ptr);
        let v1;
        if (ret[0] !== 0) {
            v1 = getStringFromWasm0(ret[0], ret[1]).slice();
            wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        }
        return v1;
    }
    /**
     * PID.
     * @returns {number}
     */
    get pid() {
        const ret = wasm.__wbg_get_wasmaudiotrack_pid(this.__wbg_ptr);
        return ret;
    }
    /**
     * `"AC3"`, `"EAC3"`, or `"MP2"`.
     * @param {string} arg0
     */
    set codec(arg0) {
        const ptr0 = passStringToWasm0(arg0, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.__wbg_set_wasmaudiotrack_codec(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * ISO 639-2 language (3 chars), or `None`.
     * @param {string | null} [arg0]
     */
    set language(arg0) {
        var ptr0 = isLikeNone(arg0) ? 0 : passStringToWasm0(arg0, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len0 = WASM_VECTOR_LEN;
        wasm.__wbg_set_wasmaudiotrack_language(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * PID.
     * @param {number} arg0
     */
    set pid(arg0) {
        wasm.__wbg_set_wasmaudiotrack_pid(this.__wbg_ptr, arg0);
    }
}
if (Symbol.dispose) WasmAudioTrack.prototype[Symbol.dispose] = WasmAudioTrack.prototype.free;

/**
 * WASM-bound Skyfire engine — thin wrapper around [`Engine`].
 *
 * # Usage from JS
 *
 * ```js
 * const engine = new WasmEngine();
 * const ch = engine.probe(tsBytes);
 * engine.init_with_channel(ch.video_pid, ch.video_codec,
 *     ch.audio_pids, ch.audio_codecs);
 * engine.feed(tsBytes);
 * engine.flush();
 * engine.finalize();
 *
 * const pcm = engine.audio_pcm();        // Uint8Array (S16LE interleaved)
 * const rate = engine.audio_sample_rate();
 * const chs = engine.audio_channels();
 *
 * for (let i = 0; i < engine.video_unit_count(); i++) {
 *     const au = engine.video_unit(i);
 *     console.log(au.bytes, au.pts_ticks);
 * }
 *
 * const codec = engine.video_config_codec();    // e.g. "avc1.640028"
 * const avcc = engine.video_config_description(); // Uint8Array
 * ```
 */
export class WasmEngine {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmEngineFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmengine_free(ptr, 0);
    }
    /**
     * Number of audio channels, or 0 if no audio.
     * @returns {number}
     */
    audio_channels() {
        const ret = wasm.wasmengine_audio_channels(this.__wbg_ptr);
        return ret;
    }
    /**
     * Decoded audio PCM as interleaved S16LE bytes (`Uint8Array`).
     * @returns {Uint8Array}
     */
    audio_pcm() {
        const ret = wasm.wasmengine_audio_pcm(this.__wbg_ptr);
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * Audio sample rate in Hz, or 0 if no audio.
     * @returns {number}
     */
    audio_sample_rate() {
        const ret = wasm.wasmengine_audio_sample_rate(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Feed raw MPEG-TS bytes into the engine.
     * @param {Uint8Array} data
     */
    feed(data) {
        const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.wasmengine_feed(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * Finalize: batch-decode accumulated audio ES to PCM, build video config.
     */
    finalize() {
        wasm.wasmengine_finalize(this.__wbg_ptr);
    }
    /**
     * Flush any partial PES packets still in the demux.
     */
    flush() {
        wasm.wasmengine_flush(this.__wbg_ptr);
    }
    /**
     * Whether the engine has produced audio PCM.
     * @returns {boolean}
     */
    has_audio() {
        const ret = wasm.wasmengine_has_audio(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Whether the engine has collected video access units.
     * @returns {boolean}
     */
    has_video() {
        const ret = wasm.wasmengine_has_video(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Initialize the engine from a channel map (typically obtained via `probe()`).
     * @param {number} video_pid
     * @param {string} video_codec
     * @param {Uint16Array} audio_pids
     * @param {string[]} audio_codecs
     */
    init_with_channel(video_pid, video_codec, audio_pids, audio_codecs) {
        const ptr0 = passStringToWasm0(video_codec, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArray16ToWasm0(audio_pids, wasm.__wbindgen_malloc);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passArrayJsValueToWasm0(audio_codecs, wasm.__wbindgen_malloc);
        const len2 = WASM_VECTOR_LEN;
        wasm.wasmengine_init_with_channel(this.__wbg_ptr, video_pid, ptr0, len0, ptr1, len1, ptr2, len2);
    }
    /**
     * Create a new, uninitialized engine.
     */
    constructor() {
        const ret = wasm.wasmengine_new();
        this.__wbg_ptr = ret;
        WasmEngineFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Probe raw MPEG-TS bytes for the channel map (PAT+PMT).
     *
     * Returns `null` if no PAT/PMT could be extracted.
     * @param {Uint8Array} data
     * @returns {ProbeResult | undefined}
     */
    probe(data) {
        const ptr0 = passArray8ToWasm0(data, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmengine_probe(this.__wbg_ptr, ptr0, len0);
        return ret === 0 ? undefined : ProbeResult.__wrap(ret);
    }
    /**
     * WebCodecs codec string (e.g. `"avc1.640028"`) or `null` if not yet available.
     * @returns {string | undefined}
     */
    video_config_codec() {
        const ret = wasm.wasmengine_video_config_codec(this.__wbg_ptr);
        let v1;
        if (ret[0] !== 0) {
            v1 = getStringFromWasm0(ret[0], ret[1]).slice();
            wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        }
        return v1;
    }
    /**
     * WebCodecs `avcC` description bytes (`Uint8Array`), or empty if not yet available.
     * @returns {Uint8Array}
     */
    video_config_description() {
        const ret = wasm.wasmengine_video_config_description(this.__wbg_ptr);
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * True when the video stream is interlaced (SPS `frame_mbs_only_flag
     * == 0`). WebCodecs cannot decode such streams — under ADR 0008 the
     * server (zenith) deinterlaces to progressive before the browser sees
     * it, so this should report `false` on a `/skyfire/<slug>` stream;
     * kept as a diagnostic.
     * @returns {boolean}
     */
    video_is_interlaced() {
        const ret = wasm.wasmengine_video_is_interlaced(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Retrieve a single video access unit by index, or `null` if out of range.
     * @param {number} index
     * @returns {WasmVideoUnit | undefined}
     */
    video_unit(index) {
        const ret = wasm.wasmengine_video_unit(this.__wbg_ptr, index);
        return ret === 0 ? undefined : WasmVideoUnit.__wrap(ret);
    }
    /**
     * Number of video access units collected.
     * @returns {number}
     */
    video_unit_count() {
        const ret = wasm.wasmengine_video_unit_count(this.__wbg_ptr);
        return ret >>> 0;
    }
}
if (Symbol.dispose) WasmEngine.prototype[Symbol.dispose] = WasmEngine.prototype.free;

/**
 * Scaffold: PCM chunk — produced in issue #31.
 */
export class WasmPcmChunk {
    static __wrap(ptr) {
        const obj = Object.create(WasmPcmChunk.prototype);
        obj.__wbg_ptr = ptr;
        WasmPcmChunkFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmPcmChunkFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmpcmchunk_free(ptr, 0);
    }
    /**
     * Number of audio channels.
     * @returns {number}
     */
    get channels() {
        const ret = wasm.__wbg_get_wasmpcmchunk_channels(this.__wbg_ptr);
        return ret;
    }
    /**
     * Sample rate in Hz (e.g. 48_000).
     * @returns {number}
     */
    get sample_rate() {
        const ret = wasm.__wbg_get_wasmpcmchunk_sample_rate(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Interleaved f32 PCM samples.
     * @returns {Float32Array}
     */
    get samples() {
        const ret = wasm.__wbg_get_wasmpcmchunk_samples(this.__wbg_ptr);
        var v1 = getArrayF32FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * Number of audio channels.
     * @param {number} arg0
     */
    set channels(arg0) {
        wasm.__wbg_set_wasmpcmchunk_channels(this.__wbg_ptr, arg0);
    }
    /**
     * Sample rate in Hz (e.g. 48_000).
     * @param {number} arg0
     */
    set sample_rate(arg0) {
        wasm.__wbg_set_wasmpcmchunk_sample_rate(this.__wbg_ptr, arg0);
    }
    /**
     * Interleaved f32 PCM samples.
     * @param {Float32Array} arg0
     */
    set samples(arg0) {
        const ptr0 = passArrayF32ToWasm0(arg0, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.__wbg_set_wasmpcmchunk_samples(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * PTS of the first sample in 90 kHz ticks, or `undefined`.
     * @returns {bigint | undefined}
     */
    get pts_ticks() {
        const ret = wasm.wasmpcmchunk_pts_ticks(this.__wbg_ptr);
        return ret[0] === 0 ? undefined : BigInt.asUintN(64, ret[1]);
    }
}
if (Symbol.dispose) WasmPcmChunk.prototype[Symbol.dispose] = WasmPcmChunk.prototype.free;

/**
 * One composited DVB subtitle cue — RGBA region bitmaps ready for JS overlay.
 *
 * Produced by the compositor from the CLUT + object pixel data in a display set.
 * JS draws each region's RGBA at (x, y) on the subtitle canvas.
 */
export class WasmSubtitleCue {
    static __wrap(ptr) {
        const obj = Object.create(WasmSubtitleCue.prototype);
        obj.__wbg_ptr = ptr;
        WasmSubtitleCueFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmSubtitleCueFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmsubtitlecue_free(ptr, 0);
    }
    /**
     * End PTS in 90 kHz ticks.
     * @returns {bigint}
     */
    get end_pts() {
        const ret = wasm.wasmsubtitlecue_end_pts(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
    /**
     * Regions in this cue, each with RGBA + screen placement.
     * @returns {WasmSubtitleRegion[]}
     */
    get regions() {
        const ret = wasm.wasmsubtitlecue_regions(this.__wbg_ptr);
        var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * PTS in 90 kHz ticks.
     * @returns {bigint}
     */
    get start_pts() {
        const ret = wasm.wasmsubtitlecue_start_pts(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
}
if (Symbol.dispose) WasmSubtitleCue.prototype[Symbol.dispose] = WasmSubtitleCue.prototype.free;

/**
 * RGBA bitmap for one subtitle region, placed on the display canvas.
 */
export class WasmSubtitleRegion {
    static __wrap(ptr) {
        const obj = Object.create(WasmSubtitleRegion.prototype);
        obj.__wbg_ptr = ptr;
        WasmSubtitleRegionFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmSubtitleRegionFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmsubtitleregion_free(ptr, 0);
    }
    /**
     * Region height in pixels.
     * @returns {number}
     */
    get height() {
        const ret = wasm.__wbg_get_wasmsubtitleregion_height(this.__wbg_ptr);
        return ret;
    }
    /**
     * RGBA pixel data, row-major, width*height*4 bytes.
     * @returns {Uint8Array}
     */
    get rgba() {
        const ret = wasm.__wbg_get_wasmsubtitleregion_rgba(this.__wbg_ptr);
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * Region width in pixels.
     * @returns {number}
     */
    get width() {
        const ret = wasm.__wbg_get_wasmsubtitleregion_width(this.__wbg_ptr);
        return ret;
    }
    /**
     * Horizontal position on the display canvas.
     * @returns {number}
     */
    get x() {
        const ret = wasm.__wbg_get_wasmsubtitleregion_x(this.__wbg_ptr);
        return ret;
    }
    /**
     * Vertical position on the display canvas.
     * @returns {number}
     */
    get y() {
        const ret = wasm.__wbg_get_wasmsubtitleregion_y(this.__wbg_ptr);
        return ret;
    }
    /**
     * Region height in pixels.
     * @param {number} arg0
     */
    set height(arg0) {
        wasm.__wbg_set_wasmsubtitleregion_height(this.__wbg_ptr, arg0);
    }
    /**
     * RGBA pixel data, row-major, width*height*4 bytes.
     * @param {Uint8Array} arg0
     */
    set rgba(arg0) {
        const ptr0 = passArray8ToWasm0(arg0, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.__wbg_set_wasmsubtitleregion_rgba(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * Region width in pixels.
     * @param {number} arg0
     */
    set width(arg0) {
        wasm.__wbg_set_wasmsubtitleregion_width(this.__wbg_ptr, arg0);
    }
    /**
     * Horizontal position on the display canvas.
     * @param {number} arg0
     */
    set x(arg0) {
        wasm.__wbg_set_wasmsubtitleregion_x(this.__wbg_ptr, arg0);
    }
    /**
     * Vertical position on the display canvas.
     * @param {number} arg0
     */
    set y(arg0) {
        wasm.__wbg_set_wasmsubtitleregion_y(this.__wbg_ptr, arg0);
    }
}
if (Symbol.dispose) WasmSubtitleRegion.prototype[Symbol.dispose] = WasmSubtitleRegion.prototype.free;

/**
 * One subtitle / teletext elementary stream.
 */
export class WasmSubtitleTrack {
    static __wrap(ptr) {
        const obj = Object.create(WasmSubtitleTrack.prototype);
        obj.__wbg_ptr = ptr;
        WasmSubtitleTrackFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    static __unwrap(jsValue) {
        if (!(jsValue instanceof WasmSubtitleTrack)) {
            return 0;
        }
        return jsValue.__destroy_into_raw();
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmSubtitleTrackFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmsubtitletrack_free(ptr, 0);
    }
    /**
     * `"DvbSubtitles"` or `"Teletext"`.
     * @returns {string}
     */
    get kind() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.__wbg_get_wasmsubtitletrack_kind(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * ISO 639-2 language (3 chars), or `None`.
     * @returns {string | undefined}
     */
    get language() {
        const ret = wasm.__wbg_get_wasmsubtitletrack_language(this.__wbg_ptr);
        let v1;
        if (ret[0] !== 0) {
            v1 = getStringFromWasm0(ret[0], ret[1]).slice();
            wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        }
        return v1;
    }
    /**
     * PID.
     * @returns {number}
     */
    get pid() {
        const ret = wasm.__wbg_get_wasmsubtitletrack_pid(this.__wbg_ptr);
        return ret;
    }
    /**
     * `"DvbSubtitles"` or `"Teletext"`.
     * @param {string} arg0
     */
    set kind(arg0) {
        const ptr0 = passStringToWasm0(arg0, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.__wbg_set_wasmsubtitletrack_kind(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * ISO 639-2 language (3 chars), or `None`.
     * @param {string | null} [arg0]
     */
    set language(arg0) {
        var ptr0 = isLikeNone(arg0) ? 0 : passStringToWasm0(arg0, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len0 = WASM_VECTOR_LEN;
        wasm.__wbg_set_wasmsubtitletrack_language(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * PID.
     * @param {number} arg0
     */
    set pid(arg0) {
        wasm.__wbg_set_wasmsubtitletrack_pid(this.__wbg_ptr, arg0);
    }
}
if (Symbol.dispose) WasmSubtitleTrack.prototype[Symbol.dispose] = WasmSubtitleTrack.prototype.free;

/**
 * Track-list produced once the first PMT has been parsed.
 */
export class WasmTrackList {
    static __wrap(ptr) {
        const obj = Object.create(WasmTrackList.prototype);
        obj.__wbg_ptr = ptr;
        WasmTrackListFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmTrackListFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmtracklist_free(ptr, 0);
    }
    /**
     * Audio tracks.
     * @returns {WasmAudioTrack[]}
     */
    get audio() {
        const ret = wasm.__wbg_get_wasmtracklist_audio(this.__wbg_ptr);
        var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * Subtitle / teletext tracks.
     * @returns {WasmSubtitleTrack[]}
     */
    get subtitles() {
        const ret = wasm.__wbg_get_wasmtracklist_subtitles(this.__wbg_ptr);
        var v1 = getArrayJsValueFromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 4, 4);
        return v1;
    }
    /**
     * Video codec string: `"H264"` or `"H265"`.
     * @returns {string}
     */
    get video_codec() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.__wbg_get_wasmtracklist_video_codec(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * PID of the video elementary stream.
     * @returns {number}
     */
    get video_pid() {
        const ret = wasm.__wbg_get_wasmtracklist_video_pid(this.__wbg_ptr);
        return ret;
    }
    /**
     * Audio tracks.
     * @param {WasmAudioTrack[]} arg0
     */
    set audio(arg0) {
        const ptr0 = passArrayJsValueToWasm0(arg0, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.__wbg_set_wasmtracklist_audio(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * Subtitle / teletext tracks.
     * @param {WasmSubtitleTrack[]} arg0
     */
    set subtitles(arg0) {
        const ptr0 = passArrayJsValueToWasm0(arg0, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.__wbg_set_wasmtracklist_subtitles(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * Video codec string: `"H264"` or `"H265"`.
     * @param {string} arg0
     */
    set video_codec(arg0) {
        const ptr0 = passStringToWasm0(arg0, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.__wbg_set_wasmtracklist_video_codec(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * PID of the video elementary stream.
     * @param {number} arg0
     */
    set video_pid(arg0) {
        wasm.__wbg_set_wasmtracklist_video_pid(this.__wbg_ptr, arg0);
    }
}
if (Symbol.dispose) WasmTrackList.prototype[Symbol.dispose] = WasmTrackList.prototype.free;

/**
 * One H.264 video access unit, ready for `VideoDecoder.decode()`.
 */
export class WasmVideoAu {
    static __wrap(ptr) {
        const obj = Object.create(WasmVideoAu.prototype);
        obj.__wbg_ptr = ptr;
        WasmVideoAuFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmVideoAuFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmvideoau_free(ptr, 0);
    }
    /**
     * Annex-B elementary-stream bytes.
     * @returns {Uint8Array}
     */
    get bytes() {
        const ret = wasm.__wbg_get_wasmvideoau_bytes(this.__wbg_ptr);
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * True when this AU contains an IDR (NAL type 5) or SPS (NAL type 7).
     * @returns {boolean}
     */
    get is_keyframe() {
        const ret = wasm.__wbg_get_wasmvideoau_is_keyframe(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Annex-B elementary-stream bytes.
     * @param {Uint8Array} arg0
     */
    set bytes(arg0) {
        const ptr0 = passArray8ToWasm0(arg0, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.__wbg_set_wasmvideoau_bytes(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * True when this AU contains an IDR (NAL type 5) or SPS (NAL type 7).
     * @param {boolean} arg0
     */
    set is_keyframe(arg0) {
        wasm.__wbg_set_wasmvideoau_is_keyframe(this.__wbg_ptr, arg0);
    }
    /**
     * DTS in 90 kHz ticks, or `undefined`.
     * @returns {bigint | undefined}
     */
    get dts_ticks() {
        const ret = wasm.wasmvideoau_dts_ticks(this.__wbg_ptr);
        return ret[0] === 0 ? undefined : BigInt.asUintN(64, ret[1]);
    }
    /**
     * PTS in 90 kHz ticks, or `undefined`.
     * @returns {bigint | undefined}
     */
    get pts_ticks() {
        const ret = wasm.wasmvideoau_pts_ticks(this.__wbg_ptr);
        return ret[0] === 0 ? undefined : BigInt.asUintN(64, ret[1]);
    }
}
if (Symbol.dispose) WasmVideoAu.prototype[Symbol.dispose] = WasmVideoAu.prototype.free;

/**
 * One H.264 video access unit surfaced to JS.
 */
export class WasmVideoUnit {
    static __wrap(ptr) {
        const obj = Object.create(WasmVideoUnit.prototype);
        obj.__wbg_ptr = ptr;
        WasmVideoUnitFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmVideoUnitFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmvideounit_free(ptr, 0);
    }
    /**
     * Elementary-stream bytes (NAL unit / picture data).
     * @returns {Uint8Array}
     */
    get bytes() {
        const ret = wasm.__wbg_get_wasmvideounit_bytes(this.__wbg_ptr);
        var v1 = getArrayU8FromWasm0(ret[0], ret[1]).slice();
        wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        return v1;
    }
    /**
     * Elementary-stream bytes (NAL unit / picture data).
     * @param {Uint8Array} arg0
     */
    set bytes(arg0) {
        const ptr0 = passArray8ToWasm0(arg0, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        wasm.__wbg_set_wasmvideounit_bytes(this.__wbg_ptr, ptr0, len0);
    }
    /**
     * PTS in 90 kHz ticks, or `undefined` if not yet known.
     * @returns {bigint | undefined}
     */
    get pts_ticks() {
        const ret = wasm.wasmvideounit_pts_ticks(this.__wbg_ptr);
        return ret[0] === 0 ? undefined : BigInt.asUintN(64, ret[1]);
    }
}
if (Symbol.dispose) WasmVideoUnit.prototype[Symbol.dispose] = WasmVideoUnit.prototype.free;
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg___wbindgen_string_get_71bb4348194e31f0: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_ea4887a5f8f9a9db: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg_wasmaudiotrack_new: function(arg0) {
            const ret = WasmAudioTrack.__wrap(arg0);
            return ret;
        },
        __wbg_wasmaudiotrack_unwrap: function(arg0) {
            const ret = WasmAudioTrack.__unwrap(arg0);
            return ret;
        },
        __wbg_wasmpcmchunk_new: function(arg0) {
            const ret = WasmPcmChunk.__wrap(arg0);
            return ret;
        },
        __wbg_wasmsubtitlecue_new: function(arg0) {
            const ret = WasmSubtitleCue.__wrap(arg0);
            return ret;
        },
        __wbg_wasmsubtitleregion_new: function(arg0) {
            const ret = WasmSubtitleRegion.__wrap(arg0);
            return ret;
        },
        __wbg_wasmsubtitletrack_new: function(arg0) {
            const ret = WasmSubtitleTrack.__wrap(arg0);
            return ret;
        },
        __wbg_wasmsubtitletrack_unwrap: function(arg0) {
            const ret = WasmSubtitleTrack.__unwrap(arg0);
            return ret;
        },
        __wbg_wasmvideoau_new: function(arg0) {
            const ret = WasmVideoAu.__wrap(arg0);
            return ret;
        },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./skyfire_wasm_bg.js": import0,
    };
}

const ProbeResultFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_proberesult_free(ptr, 1));
const SkyfireBridgeFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_skyfirebridge_free(ptr, 1));
const WasmAudioTrackFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmaudiotrack_free(ptr, 1));
const WasmEngineFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmengine_free(ptr, 1));
const WasmPcmChunkFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmpcmchunk_free(ptr, 1));
const WasmSubtitleCueFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmsubtitlecue_free(ptr, 1));
const WasmSubtitleRegionFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmsubtitleregion_free(ptr, 1));
const WasmSubtitleTrackFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmsubtitletrack_free(ptr, 1));
const WasmTrackListFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmtracklist_free(ptr, 1));
const WasmVideoAuFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmvideoau_free(ptr, 1));
const WasmVideoUnitFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmvideounit_free(ptr, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

function getArrayF32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getFloat32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayJsValueFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    const mem = getDataViewMemory0();
    const result = [];
    for (let i = ptr; i < ptr + 4 * len; i += 4) {
        result.push(wasm.__wbindgen_externrefs.get(mem.getUint32(i, true)));
    }
    wasm.__externref_drop_slice(ptr, len);
    return result;
}

function getArrayU16FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint16ArrayMemory0().subarray(ptr / 2, ptr / 2 + len);
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

let cachedFloat32ArrayMemory0 = null;
function getFloat32ArrayMemory0() {
    if (cachedFloat32ArrayMemory0 === null || cachedFloat32ArrayMemory0.byteLength === 0) {
        cachedFloat32ArrayMemory0 = new Float32Array(wasm.memory.buffer);
    }
    return cachedFloat32ArrayMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint16ArrayMemory0 = null;
function getUint16ArrayMemory0() {
    if (cachedUint16ArrayMemory0 === null || cachedUint16ArrayMemory0.byteLength === 0) {
        cachedUint16ArrayMemory0 = new Uint16Array(wasm.memory.buffer);
    }
    return cachedUint16ArrayMemory0;
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function passArray16ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 2, 2) >>> 0;
    getUint16ArrayMemory0().set(arg, ptr / 2);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArray8ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 1, 1) >>> 0;
    getUint8ArrayMemory0().set(arg, ptr / 1);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayF32ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 4, 4) >>> 0;
    getFloat32ArrayMemory0().set(arg, ptr / 4);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayJsValueToWasm0(array, malloc) {
    const ptr = malloc(array.length * 4, 4) >>> 0;
    for (let i = 0; i < array.length; i++) {
        const add = addToExternrefTable0(array[i]);
        getDataViewMemory0().setUint32(ptr + 4 * i, add, true);
    }
    WASM_VECTOR_LEN = array.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedFloat32ArrayMemory0 = null;
    cachedUint16ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('skyfire_wasm_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
