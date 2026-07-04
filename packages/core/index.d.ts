export function initSkyfire(): Promise<void>;

/** MPEG-TS PTS clock frequency (90 kHz). */
export const PTS_HZ: 90_000;

/** Convert 90 kHz PTS ticks to microseconds. */
export function ticksToMicros(ticks: number | bigint): number;

export interface WasmAudioTrack {
  pid: number;
  codec: "AC3" | "EAC3" | "MP2";
  language?: string;
}

export interface WasmSubtitleTrack {
  pid: number;
  kind: string;
  language?: string;
}

export interface TrackList {
  video_pid: number;
  video_codec: string;
  audio: WasmAudioTrack[];
  subtitles: WasmSubtitleTrack[];
}

export interface VideoAu {
  bytes: Uint8Array;
  pts_ticks?: bigint;
  dts_ticks?: bigint;
  is_keyframe: boolean;
}

export interface MediaSegment {
  bytes: Uint8Array;
  base_media_decode_time: bigint;
  sample_count: number;
}

export interface PcmChunk {
  samples: Float32Array;
  sample_rate: number;
  channels: number;
  pts_ticks?: bigint;
}

export interface SubtitleRegion {
  x: number;
  y: number;
  width: number;
  height: number;
  rgba: Uint8Array;
}

export interface SubtitleCue {
  start_pts: bigint;
  end_pts: bigint;
  regions: SubtitleRegion[];
}

export class SkyfireBridge {
  constructor();
  feed(bytes: Uint8Array): void;
  flush(): void;
  track_list(): TrackList | undefined;
  select_audio(pid: number): void;
  select_subtitle(pid?: number | null): void;
  set_audio_downmix(enabled: boolean): void;
  set_playing(playing: boolean): void;
  audio_native_channels(): number;
  take_video_aus(): VideoAu[];
  video_codec(): string | undefined;
  video_config_description(): Uint8Array;
  video_init_segment(): Uint8Array;
  take_video_media_segment(): MediaSegment | undefined;
  take_audio_pcm(): PcmChunk[];
  take_subtitle_cues(): SubtitleCue[];
  pcr_pts(): bigint | undefined;
}
