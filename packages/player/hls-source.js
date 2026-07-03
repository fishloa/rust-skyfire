/**
 * HLS playlist parser and byte-source abstractions — pure I/O wrappers.
 *
 * @module hls-source
 */

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * DirectSource wraps a single HTTP fetch and delegates reads to the response
 * body reader. Mirrors the ReadableStreamDefaultReader interface.
 */
export class DirectSource {
  constructor(url, { signal, fetchImpl = fetch } = {}) {
    this._url = url;
    this._signal = signal;
    this._fetchImpl = fetchImpl;
    this._reader = null;
    /** @type {boolean} Always false for a finite/direct stream. */
    this.isLive = false;
  }

  async read() {
    if (!this._reader) {
      const fetchImpl = this._fetchImpl;
      const resp = await fetchImpl(this._url, { signal: this._signal });
      if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
      this._reader = resp.body.getReader();
    }
    return this._reader.read();
  }

  cancel() {
    if (this._reader) {
      this._reader.cancel();
      this._reader = null;
    }
  }
}

/**
 * HlsSource fetches an HLS playlist and returns one segment's bytes per
 * non-done read(). Supports both VOD (ENDLIST) and live playlists.
 */
export class HlsSource {
  constructor(url, { signal, fetchImpl = fetch } = {}) {
    this._url = url;
    this._signal = signal;
    this._fetchImpl = fetchImpl;
    this._lastSeq = -1;
    this._pending = [];
    this._endList = false;
    this._targetDuration = 2;
    this._primed = false;
    /** @type {boolean} Updated after the first successful playlist fetch. */
    this.isLive = false;
  }

  async _refreshPlaylist() {
    const fetchImpl = this._fetchImpl;
    let playlistUrl = this._url;
    let resp = await fetchImpl(playlistUrl, { signal: this._signal });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    let text = await resp.text();
    let parsed = parsePlaylist(text, resp.url || playlistUrl);

    if (parsed.kind === "master") {
      if (!parsed.variants || parsed.variants.length === 0) {
        throw new Error("HLS master playlist has no variants");
      }
      playlistUrl = parsed.variants[0].uri;
      resp = await fetchImpl(playlistUrl, { signal: this._signal });
      if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
      text = await resp.text();
      parsed = parsePlaylist(text, resp.url || playlistUrl);
      if (parsed.kind !== "media") {
        throw new Error("HLS variant playlist is not a media playlist");
      }
    }

    this._targetDuration = parsed.targetDuration || 2;
    this._endList = parsed.endList;
    this.isLive = !parsed.endList;

    for (const seg of parsed.segments) {
      if (seg.seq > this._lastSeq) {
        this._pending.push(seg);
        this._lastSeq = seg.seq;
      }
    }

    this._primed = true;
  }

  async read() {
    const fetchImpl = this._fetchImpl;
    // eslint-disable-next-line no-constant-condition
    while (true) {
      if (this._pending.length > 0) {
        const seg = this._pending.shift();
        const resp = await fetchImpl(seg.uri, { signal: this._signal });
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const buf = await resp.arrayBuffer();
        return { done: false, value: new Uint8Array(buf) };
      }

      if (this._primed && this._endList) {
        return { done: true, value: undefined };
      }

      if (this._primed) {
        // Live: wait half the target duration before refreshing
        await sleep(Math.max((this._targetDuration || 2) / 2, 0.5) * 1000);
      }

      await this._refreshPlaylist();
    }
  }

  cancel() {
    this._pending = [];
  }
}

/**
 * Returns true if the url looks like an HLS playlist URL (.m3u8), unless
 * overridden by opts.hls.
 *
 * @param {string} url
 * @param {{ hls?: boolean }} [opts]
 * @returns {boolean}
 */
export function isHlsUrl(url, opts = {}) {
  if (typeof opts.hls === "boolean") return opts.hls;
  return /\.m3u8(\?|$)/i.test(url);
}

/**
 * Create the appropriate source for a URL.
 *
 * @param {string} url
 * @param {{ signal?: AbortSignal, fetchImpl?: Function, hls?: boolean }} [opts]
 * @returns {DirectSource|HlsSource}
 */
export function makeSource(url, { signal, fetchImpl, hls } = {}) {
  return isHlsUrl(url, { hls })
    ? new HlsSource(url, { signal, fetchImpl })
    : new DirectSource(url, { signal, fetchImpl });
}

/**
 * Parse an HLS media or master playlist.
 *
 * @param {string} text     - Raw playlist text.
 * @param {string} baseUrl  - Base URL used to resolve relative URIs.
 * @returns {{ kind: "master", variants: Array<{ uri: string, bandwidth: number }> }
 *          |{ kind: "media", mediaSequence: number, targetDuration: number,
 *             endList: boolean, segments: Array<{ uri: string, duration: number,
 *             seq: number, discontinuity: boolean }> }}
 * @throws {Error} if the first non-empty line is not "#EXTM3U".
 */
export function parsePlaylist(text, baseUrl) {
  const lines = text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l.length > 0);

  if (lines.length === 0 || lines[0] !== "#EXTM3U") {
    throw new Error('HLS playlist must start with #EXTM3U');
  }

  // Determine playlist type: master if any #EXT-X-STREAM-INF present.
  const isMaster = lines.some((l) => l.startsWith("#EXT-X-STREAM-INF"));

  if (isMaster) {
    const variants = [];
    for (let i = 1; i < lines.length; i++) {
      const line = lines[i];
      if (line.startsWith("#EXT-X-STREAM-INF")) {
        const bwMatch = line.match(/BANDWIDTH=(\d+)/);
        const bandwidth = bwMatch ? parseInt(bwMatch[1], 10) : 0;
        // Next non-empty line is the variant URI
        const uriLine = lines[i + 1];
        if (uriLine && !uriLine.startsWith("#")) {
          variants.push({
            uri: new URL(uriLine, baseUrl).href,
            bandwidth,
          });
          i++; // skip the URI line
        }
      }
    }
    return { kind: "master", variants };
  }

  // Media playlist
  let mediaSequence = 0;
  let targetDuration = 0;
  let endList = false;
  const segments = [];

  let pendingDuration = 0;
  let pendingDiscontinuity = false;
  let seq = 0;
  let seqInitialized = false;

  for (let i = 1; i < lines.length; i++) {
    const line = lines[i];

    if (line.startsWith("#EXT-X-MEDIA-SEQUENCE:")) {
      mediaSequence = parseInt(line.slice("#EXT-X-MEDIA-SEQUENCE:".length), 10);
      seq = mediaSequence;
      seqInitialized = true;
    } else if (line.startsWith("#EXT-X-TARGETDURATION:")) {
      targetDuration = parseInt(line.slice("#EXT-X-TARGETDURATION:".length), 10);
    } else if (line.startsWith("#EXTINF:")) {
      const val = line.slice("#EXTINF:".length);
      pendingDuration = parseFloat(val.split(",")[0]);
    } else if (line === "#EXT-X-DISCONTINUITY") {
      pendingDiscontinuity = true;
    } else if (line === "#EXT-X-ENDLIST") {
      endList = true;
    } else if (!line.startsWith("#")) {
      // Segment URI
      segments.push({
        uri: new URL(line, baseUrl).href,
        duration: pendingDuration,
        seq,
        discontinuity: pendingDiscontinuity,
      });
      seq++;
      pendingDuration = 0;
      pendingDiscontinuity = false;
    }
    // Any other tag (#EXT-X-VERSION, #EXT-X-ALLOW-CACHE, etc.) is silently ignored.
  }

  return { kind: "media", mediaSequence, targetDuration, endList, segments };
}
