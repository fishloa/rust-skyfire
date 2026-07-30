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
  /**
   * PID whose audio is genuinely being decoded, or `null` before any audio has
   * decoded. Distinct from `selectedAudio`, which is only the request — see
   * issue #89, where reporting the request made a broken track switch look
   * successful.
   */
  decodedAudioPid: number | null;
  /**
   * Channel count of the decoded audio before downmix, from the decoder itself.
   * Changes when a track switch lands on a different layout.
   */
  nativeChannels?: number;
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

/** What changed between two track lists, delivered alongside `tracks`. */
export interface SkyfireTrackDiff {
  added: Array<WasmAudioTrack | WasmSubtitleTrack>;
  removed: Array<WasmAudioTrack | WasmSubtitleTrack>;
  changed: Array<WasmAudioTrack | WasmSubtitleTrack>;
  /** Present when the selected audio PID vanished and a fallback was chosen. */
  reselected?: { from: number; to: number };
}

export function trackSignature(tl: TrackList | null | undefined): string;
export function diffTracks(
  prev: TrackList | null | undefined,
  next: TrackList | null | undefined,
): SkyfireTrackDiff;
export function pickFallbackAudio(
  audio: WasmAudioTrack[] | null | undefined,
  lostPid: number,
): number | null;

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
  on(event: "tracks", cb: (tracks: TrackList, diff: SkyfireTrackDiff) => void): void;
  on(event: "stats", cb: (stats: SkyfireStats) => void): void;
  on(event: "error", cb: (err: SkyfireErrorEvent) => void): void;
  on(event: "ended", cb: (stats: SkyfireEndedStats) => void): void;
  on<E extends SkyfireEvent>(
    event: E,
    cb: (data: SkyfireEventMap[E]) => void,
  ): void;
  destroy(): void;
}

export function languageName(
  code: string | null | undefined,
  locale?: string,
  overrides?: Record<string, string>,
): string | null;

export function resolveLocale(el: Element | null | undefined): string;

export interface SkyfireFullscreenChangeDetail {
  fullscreen: boolean;
  mode: "native" | "pseudo";
}

/** The `<skyfire-player>` custom element. */
export interface SkyfirePlayerElement extends HTMLElement {
  readonly isFullscreen: boolean;
  enterFullscreen(): Promise<void>;
  exitFullscreen(): Promise<void>;
  toggleFullscreen(): Promise<void>;
}

declare global {
  interface HTMLElementTagNameMap {
    "skyfire-player": SkyfirePlayerElement;
  }
  interface HTMLElementEventMap {
    "sf-fullscreenchange": CustomEvent<SkyfireFullscreenChangeDetail>;
    "sf-tracks-changed": CustomEvent<SkyfireTrackDiff>;
  }
}
