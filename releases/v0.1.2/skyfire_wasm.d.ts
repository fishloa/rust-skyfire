/* tslint:disable */
/* eslint-disable */

/**
 * Result of probing MPEG-TS bytes for the channel map (PAT+PMT).
 */
export class ProbeResult {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Video codec identifier: `"H264"` or `"H265"`.
     */
    video_codec: string;
    /**
     * PID of the video elementary stream.
     */
    video_pid: number;
    /**
     * Audio codec strings, parallel to `audio_pids`.
     */
    readonly audio_codecs: string[];
    /**
     * PIDs of audio elementary streams.
     */
    readonly audio_pids: Uint16Array;
}

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
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Push a raw TS chunk into the bridge.
     *
     * Demuxes PAT/PMT on the fly and accumulates video AUs.
     */
    feed(bytes: Uint8Array): void;
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
    flush(): void;
    /**
     * Create a new, empty bridge.
     */
    constructor();
    /**
     * Latest PCR-derived clock value in 90 kHz ticks.
     *
     * The `EsDemux` / `SiDemux` layer does not separately surface PCR values;
     * we derive this from the most recently seen video or selected-audio PTS,
     * which is within one PCR interval (~40 ms for DVB) of the true PCR.
     * A future issue can replace this with raw PCR extraction if sub-millisecond
     * accuracy is required (verified 2026-06-22).
     */
    pcr_pts(): bigint | undefined;
    /**
     * Select which audio PID to route and decode.
     *
     * If the PID changes, the AC-3/E-AC-3 and MPEG audio decoder states are
     * reset so the new stream decodes cleanly (PTS continuity is handled in
     * issue #33).
     */
    select_audio(pid: number): void;
    /**
     * Select a subtitle PID, or `None` to disable subtitles.
     *
     * Calling this clears any buffered subtitle cues from the previously
     * selected PID (or disables subtitle output when `pid` is `None`).
     */
    select_subtitle(pid?: number | null): void;
    /**
     * Set the play/pause state (stored; gates nothing critical yet).
     */
    set_playing(playing: boolean): void;
    /**
     * Drain all decoded PCM chunks produced since the last call.
     *
     * Each chunk corresponds to one audio access unit decoded from the
     * selected audio PID.  Samples are interleaved f32 (WebAudio-ready).
     */
    take_audio_pcm(): WasmPcmChunk[];
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
     */
    take_subtitle_cues(): WasmSubtitleCue[];
    /**
     * Drain all completed video access units since the last call.
     *
     * Returns Annex-B bytes with PTS/DTS and a keyframe flag.
     */
    take_video_aus(): WasmVideoAu[];
    /**
     * Returns the track list once a PMT has been parsed, or `None`.
     */
    track_list(): WasmTrackList | undefined;
    /**
     * WebCodecs codec string (e.g. `"avc1.640028"`) once SPS has been seen.
     *
     * Returns `None` until sufficient video AUs have been fed to extract an SPS.
     * Once extracted, the config is cached and survives `take_video_aus()` drains.
     */
    video_codec(): string | undefined;
}

/**
 * One audio elementary stream.
 */
export class WasmAudioTrack {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * `"AC3"`, `"EAC3"`, or `"MP2"`.
     */
    codec: string;
    /**
     * ISO 639-2 language (3 chars), or `None`.
     */
    get language(): string | undefined;
    /**
     * ISO 639-2 language (3 chars), or `None`.
     */
    set language(value: string | null | undefined);
    /**
     * PID.
     */
    pid: number;
}

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
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Number of audio channels, or 0 if no audio.
     */
    audio_channels(): number;
    /**
     * Decoded audio PCM as interleaved S16LE bytes (`Uint8Array`).
     */
    audio_pcm(): Uint8Array;
    /**
     * Audio sample rate in Hz, or 0 if no audio.
     */
    audio_sample_rate(): number;
    /**
     * Feed raw MPEG-TS bytes into the engine.
     */
    feed(data: Uint8Array): void;
    /**
     * Finalize: batch-decode accumulated audio ES to PCM, build video config.
     */
    finalize(): void;
    /**
     * Flush any partial PES packets still in the demux.
     */
    flush(): void;
    /**
     * Whether the engine has produced audio PCM.
     */
    has_audio(): boolean;
    /**
     * Whether the engine has collected video access units.
     */
    has_video(): boolean;
    /**
     * Initialize the engine from a channel map (typically obtained via `probe()`).
     */
    init_with_channel(video_pid: number, video_codec: string, audio_pids: Uint16Array, audio_codecs: string[]): void;
    /**
     * Create a new, uninitialized engine.
     */
    constructor();
    /**
     * Probe raw MPEG-TS bytes for the channel map (PAT+PMT).
     *
     * Returns `null` if no PAT/PMT could be extracted.
     */
    probe(data: Uint8Array): ProbeResult | undefined;
    /**
     * WebCodecs codec string (e.g. `"avc1.640028"`) or `null` if not yet available.
     */
    video_config_codec(): string | undefined;
    /**
     * WebCodecs `avcC` description bytes (`Uint8Array`), or empty if not yet available.
     */
    video_config_description(): Uint8Array;
    /**
     * True when the video stream is interlaced (SPS `frame_mbs_only_flag
     * == 0`). WebCodecs cannot decode such streams — under ADR 0008 the
     * server (zenith) deinterlaces to progressive before the browser sees
     * it, so this should report `false` on a `/skyfire/<slug>` stream;
     * kept as a diagnostic.
     */
    video_is_interlaced(): boolean;
    /**
     * Retrieve a single video access unit by index, or `null` if out of range.
     */
    video_unit(index: number): WasmVideoUnit | undefined;
    /**
     * Number of video access units collected.
     */
    video_unit_count(): number;
}

/**
 * Scaffold: PCM chunk — produced in issue #31.
 */
export class WasmPcmChunk {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Number of audio channels.
     */
    channels: number;
    /**
     * Sample rate in Hz (e.g. 48_000).
     */
    sample_rate: number;
    /**
     * Interleaved f32 PCM samples.
     */
    samples: Float32Array;
    /**
     * PTS of the first sample in 90 kHz ticks, or `undefined`.
     */
    readonly pts_ticks: bigint | undefined;
}

/**
 * One composited DVB subtitle cue — RGBA region bitmaps ready for JS overlay.
 *
 * Produced by the compositor from the CLUT + object pixel data in a display set.
 * JS draws each region's RGBA at (x, y) on the subtitle canvas.
 */
export class WasmSubtitleCue {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * End PTS in 90 kHz ticks.
     */
    readonly end_pts: bigint;
    /**
     * Regions in this cue, each with RGBA + screen placement.
     */
    readonly regions: WasmSubtitleRegion[];
    /**
     * PTS in 90 kHz ticks.
     */
    readonly start_pts: bigint;
}

/**
 * RGBA bitmap for one subtitle region, placed on the display canvas.
 */
export class WasmSubtitleRegion {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Region height in pixels.
     */
    height: number;
    /**
     * RGBA pixel data, row-major, width*height*4 bytes.
     */
    rgba: Uint8Array;
    /**
     * Region width in pixels.
     */
    width: number;
    /**
     * Horizontal position on the display canvas.
     */
    x: number;
    /**
     * Vertical position on the display canvas.
     */
    y: number;
}

/**
 * One subtitle / teletext elementary stream.
 */
export class WasmSubtitleTrack {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * `"DvbSubtitles"` or `"Teletext"`.
     */
    kind: string;
    /**
     * ISO 639-2 language (3 chars), or `None`.
     */
    get language(): string | undefined;
    /**
     * ISO 639-2 language (3 chars), or `None`.
     */
    set language(value: string | null | undefined);
    /**
     * PID.
     */
    pid: number;
}

/**
 * Track-list produced once the first PMT has been parsed.
 */
export class WasmTrackList {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Audio tracks.
     */
    audio: WasmAudioTrack[];
    /**
     * Subtitle / teletext tracks.
     */
    subtitles: WasmSubtitleTrack[];
    /**
     * Video codec string: `"H264"` or `"H265"`.
     */
    video_codec: string;
    /**
     * PID of the video elementary stream.
     */
    video_pid: number;
}

/**
 * One H.264 video access unit, ready for `VideoDecoder.decode()`.
 */
export class WasmVideoAu {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Annex-B elementary-stream bytes.
     */
    bytes: Uint8Array;
    /**
     * True when this AU contains an IDR (NAL type 5) or SPS (NAL type 7).
     */
    is_keyframe: boolean;
    /**
     * DTS in 90 kHz ticks, or `undefined`.
     */
    readonly dts_ticks: bigint | undefined;
    /**
     * PTS in 90 kHz ticks, or `undefined`.
     */
    readonly pts_ticks: bigint | undefined;
}

/**
 * One H.264 video access unit surfaced to JS.
 */
export class WasmVideoUnit {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Elementary-stream bytes (NAL unit / picture data).
     */
    bytes: Uint8Array;
    /**
     * PTS in 90 kHz ticks, or `undefined` if not yet known.
     */
    readonly pts_ticks: bigint | undefined;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_get_proberesult_video_codec: (a: number) => [number, number];
    readonly __wbg_get_proberesult_video_pid: (a: number) => number;
    readonly __wbg_get_wasmaudiotrack_language: (a: number) => [number, number];
    readonly __wbg_get_wasmaudiotrack_pid: (a: number) => number;
    readonly __wbg_get_wasmpcmchunk_channels: (a: number) => number;
    readonly __wbg_get_wasmpcmchunk_sample_rate: (a: number) => number;
    readonly __wbg_get_wasmpcmchunk_samples: (a: number) => [number, number];
    readonly __wbg_get_wasmsubtitleregion_height: (a: number) => number;
    readonly __wbg_get_wasmsubtitleregion_rgba: (a: number) => [number, number];
    readonly __wbg_get_wasmsubtitleregion_width: (a: number) => number;
    readonly __wbg_get_wasmsubtitleregion_x: (a: number) => number;
    readonly __wbg_get_wasmsubtitleregion_y: (a: number) => number;
    readonly __wbg_get_wasmtracklist_audio: (a: number) => [number, number];
    readonly __wbg_get_wasmtracklist_subtitles: (a: number) => [number, number];
    readonly __wbg_get_wasmvideoau_bytes: (a: number) => [number, number];
    readonly __wbg_get_wasmvideoau_is_keyframe: (a: number) => number;
    readonly __wbg_get_wasmvideounit_bytes: (a: number) => [number, number];
    readonly __wbg_proberesult_free: (a: number, b: number) => void;
    readonly __wbg_set_proberesult_video_codec: (a: number, b: number, c: number) => void;
    readonly __wbg_set_proberesult_video_pid: (a: number, b: number) => void;
    readonly __wbg_set_wasmaudiotrack_language: (a: number, b: number, c: number) => void;
    readonly __wbg_set_wasmaudiotrack_pid: (a: number, b: number) => void;
    readonly __wbg_set_wasmpcmchunk_channels: (a: number, b: number) => void;
    readonly __wbg_set_wasmpcmchunk_sample_rate: (a: number, b: number) => void;
    readonly __wbg_set_wasmpcmchunk_samples: (a: number, b: number, c: number) => void;
    readonly __wbg_set_wasmsubtitleregion_height: (a: number, b: number) => void;
    readonly __wbg_set_wasmsubtitleregion_width: (a: number, b: number) => void;
    readonly __wbg_set_wasmsubtitleregion_x: (a: number, b: number) => void;
    readonly __wbg_set_wasmsubtitleregion_y: (a: number, b: number) => void;
    readonly __wbg_set_wasmtracklist_audio: (a: number, b: number, c: number) => void;
    readonly __wbg_set_wasmtracklist_subtitles: (a: number, b: number, c: number) => void;
    readonly __wbg_set_wasmvideoau_bytes: (a: number, b: number, c: number) => void;
    readonly __wbg_set_wasmvideoau_is_keyframe: (a: number, b: number) => void;
    readonly __wbg_set_wasmvideounit_bytes: (a: number, b: number, c: number) => void;
    readonly __wbg_skyfirebridge_free: (a: number, b: number) => void;
    readonly __wbg_wasmaudiotrack_free: (a: number, b: number) => void;
    readonly __wbg_wasmengine_free: (a: number, b: number) => void;
    readonly __wbg_wasmpcmchunk_free: (a: number, b: number) => void;
    readonly __wbg_wasmsubtitlecue_free: (a: number, b: number) => void;
    readonly __wbg_wasmsubtitleregion_free: (a: number, b: number) => void;
    readonly __wbg_wasmsubtitletrack_free: (a: number, b: number) => void;
    readonly __wbg_wasmtracklist_free: (a: number, b: number) => void;
    readonly __wbg_wasmvideoau_free: (a: number, b: number) => void;
    readonly __wbg_wasmvideounit_free: (a: number, b: number) => void;
    readonly proberesult_audio_codecs: (a: number) => [number, number];
    readonly proberesult_audio_pids: (a: number) => [number, number];
    readonly skyfirebridge_feed: (a: number, b: number, c: number) => void;
    readonly skyfirebridge_flush: (a: number) => void;
    readonly skyfirebridge_new: () => number;
    readonly skyfirebridge_pcr_pts: (a: number) => [number, bigint];
    readonly skyfirebridge_select_audio: (a: number, b: number) => void;
    readonly skyfirebridge_select_subtitle: (a: number, b: number) => void;
    readonly skyfirebridge_set_playing: (a: number, b: number) => void;
    readonly skyfirebridge_take_audio_pcm: (a: number) => [number, number];
    readonly skyfirebridge_take_subtitle_cues: (a: number) => [number, number];
    readonly skyfirebridge_take_video_aus: (a: number) => [number, number];
    readonly skyfirebridge_track_list: (a: number) => number;
    readonly skyfirebridge_video_codec: (a: number) => [number, number];
    readonly wasmengine_audio_channels: (a: number) => number;
    readonly wasmengine_audio_pcm: (a: number) => [number, number];
    readonly wasmengine_audio_sample_rate: (a: number) => number;
    readonly wasmengine_feed: (a: number, b: number, c: number) => void;
    readonly wasmengine_finalize: (a: number) => void;
    readonly wasmengine_flush: (a: number) => void;
    readonly wasmengine_has_audio: (a: number) => number;
    readonly wasmengine_has_video: (a: number) => number;
    readonly wasmengine_init_with_channel: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => void;
    readonly wasmengine_new: () => number;
    readonly wasmengine_probe: (a: number, b: number, c: number) => number;
    readonly wasmengine_video_config_codec: (a: number) => [number, number];
    readonly wasmengine_video_config_description: (a: number) => [number, number];
    readonly wasmengine_video_is_interlaced: (a: number) => number;
    readonly wasmengine_video_unit: (a: number, b: number) => number;
    readonly wasmengine_video_unit_count: (a: number) => number;
    readonly wasmsubtitlecue_end_pts: (a: number) => bigint;
    readonly wasmsubtitlecue_regions: (a: number) => [number, number];
    readonly wasmsubtitlecue_start_pts: (a: number) => bigint;
    readonly wasmvideoau_dts_ticks: (a: number) => [number, bigint];
    readonly __wbg_get_wasmaudiotrack_codec: (a: number) => [number, number];
    readonly __wbg_get_wasmsubtitletrack_kind: (a: number) => [number, number];
    readonly __wbg_get_wasmtracklist_video_codec: (a: number) => [number, number];
    readonly __wbg_get_wasmsubtitletrack_language: (a: number) => [number, number];
    readonly __wbg_set_wasmsubtitletrack_language: (a: number, b: number, c: number) => void;
    readonly wasmpcmchunk_pts_ticks: (a: number) => [number, bigint];
    readonly wasmvideoau_pts_ticks: (a: number) => [number, bigint];
    readonly wasmvideounit_pts_ticks: (a: number) => [number, bigint];
    readonly __wbg_get_wasmsubtitletrack_pid: (a: number) => number;
    readonly __wbg_get_wasmtracklist_video_pid: (a: number) => number;
    readonly __wbg_set_wasmaudiotrack_codec: (a: number, b: number, c: number) => void;
    readonly __wbg_set_wasmsubtitleregion_rgba: (a: number, b: number, c: number) => void;
    readonly __wbg_set_wasmsubtitletrack_kind: (a: number, b: number, c: number) => void;
    readonly __wbg_set_wasmtracklist_video_codec: (a: number, b: number, c: number) => void;
    readonly __wbg_set_wasmsubtitletrack_pid: (a: number, b: number) => void;
    readonly __wbg_set_wasmtracklist_video_pid: (a: number, b: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __externref_drop_slice: (a: number, b: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
