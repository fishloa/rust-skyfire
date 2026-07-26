import type { TrackList } from "@firemedia/skyfire-core";

export interface SkyfirePlayerOptions {
  streamUrl: string;
  audioPid?: number;
  subtitlePid?: number;
  muted?: boolean;
  forceMse?: boolean;
}

export type SkyfireEvent = "tracks" | "stats" | "error" | "ended";

export class SkyfirePlayer {
  constructor(canvas: HTMLCanvasElement, opts: SkyfirePlayerOptions);
  init(): Promise<void>;
  play(): void;
  pause(): void;
  selectAudio(pid: number): void;
  selectSubtitle(pid: number | null): void;
  tracks(): TrackList | null;
  on(event: SkyfireEvent, cb: (data: unknown) => void): void;
  destroy(): void;
}

export function languageName(
  code: string | null | undefined,
  locale?: string,
  overrides?: Record<string, string>,
): string | null;

export function resolveLocale(el: Element | null): string;
