/**
 * HLS playlist parser — pure function, no I/O.
 *
 * @module hls-source
 */

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
