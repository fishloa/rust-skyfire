import type { TrackList, WasmAudioTrack, WasmSubtitleTrack } from "@firemedia/skyfire-core";

export interface SkyfirePlayerOptions {
  streamUrl: string;
  audioPid?: number;
  subtitlePid?: number;
  muted?: boolean;
  forceMse?: boolean;
}

export type SkyfireEvent = "tracks" | "stats" | "error" | "ended";

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

export class SkyfirePlayer {
  constructor(canvas: HTMLCanvasElement, opts: SkyfirePlayerOptions);
  init(): Promise<void>;
  play(): void;
  pause(): void;
  selectAudio(pid: number): void;
  selectSubtitle(pid: number | null): void;
  tracks(): TrackList | null;
  on(event: "tracks", cb: (tracks: TrackList, diff: SkyfireTrackDiff) => void): void;
  on(event: Exclude<SkyfireEvent, "tracks">, cb: (data: unknown) => void): void;
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
