import type {
  TrackList,
  WasmAudioTrack,
  WasmSubtitleTrack,
} from "@firemedia/skyfire-core";

export interface SkyfirePlayerOptions {
  streamUrl: string;
  audioPid?: number;
  subtitlePid?: number;
  muted?: boolean;
  forceMse?: boolean;
}

export type SkyfireEvent = "tracks" | "stats" | "error" | "ended";

/** Track summary carried on `stats.tracks` (flat, already PMT-resolved). */
export interface SkyfireStatsTracks {
  audio: WasmAudioTrack[];
  subtitle: WasmSubtitleTrack[];
}

/**
 * Counters + current state emitted on every `stats` event. The player emits a
 * shallow copy of its internal stats object, so the fields present depend on
 * how far the stream has progressed: the counters below are always initialised,
 * the optional ones only appear once the corresponding path has run.
 */
export interface SkyfireStats {
  /** Video frames returned by the decoder. */
  decoded: number;
  /** Video frames actually drawn to the canvas. */
  drawn: number;
  /** Video frames dropped by the presenter (late vs the audio clock). */
  dropped: number;
  /** Last drawn frame size. */
  w: number;
  h: number;
  /** H.264 access units handed to the video path. */
  aus: number;
  path: string;
  audioChunks: number;
  audioSamples: number;
  /** Frames the AudioWorklet reports as played out. */
  audioFrames: number;
  /** Seconds of audio played out (`audioFrames / sampleRate`). */
  audioSec: number;
  /** Video presentation time minus audio clock, milliseconds. */
  avSkewMs: number;
  /** Which video path is live; `""` until the first configure. */
  videoPath: "" | "webcodecs" | "mse";
  mseSegments: number;
  videoCurrentTime: number;
  tracks: SkyfireStatsTracks;
  /** PID requested via `selectAudio`, or `null` before any selection. */
  selectedAudio: number | null;
  /** PID the bridge is actually decoding, or `null`. */
  decodedAudioPid: number | null;
  subCues: number;
  /** AudioWorklet output-buffer underruns (absent until audio starts). */
  audioUnderruns?: number;
  /** PCM chunks dropped before the audio graph was ready. */
  audioDropped?: number;
  /** Human-readable status line, present on status-driven emits. */
  status?: string;
  /** `true` on the final emit after the stream ends. */
  done?: boolean;
}

/** Terminal stats emitted alongside `ended`. */
export type SkyfireEndedStats = SkyfireStats & { done: true };

/**
 * Payload of the `error` event. Note this is NOT an `Error` instance: the
 * player wraps the failure so the originating exception stays inspectable.
 */
export interface SkyfireErrorEvent {
  /** Player-supplied description, with the cause's message appended. */
  message: string;
  /** Whatever was thrown, if the failure originated in an exception. */
  cause?: unknown;
}

/** Event name → payload type. Exposed so consumers can type shared handlers. */
export interface SkyfireEventMap {
  tracks: TrackList;
  stats: SkyfireStats;
  error: SkyfireErrorEvent;
  ended: SkyfireEndedStats;
}

export class SkyfirePlayer {
  constructor(canvas: HTMLCanvasElement, opts: SkyfirePlayerOptions);
  init(): Promise<void>;
  play(): void;
  pause(): void;
  selectAudio(pid: number): void;
  selectSubtitle(pid: number | null): void;
  tracks(): TrackList | null;
  on(event: "tracks", cb: (tracks: TrackList) => void): void;
  on(event: "stats", cb: (stats: SkyfireStats) => void): void;
  on(event: "error", cb: (err: SkyfireErrorEvent) => void): void;
  on(event: "ended", cb: (stats: SkyfireEndedStats) => void): void;
  on<E extends SkyfireEvent>(
    event: E,
    cb: (data: SkyfireEventMap[E]) => void,
  ): void;
  destroy(): void;
}
