/**
 * Line-buffered SSE consumer over Node's built-in fetch + ReadableStream.
 * Yields one parsed JSON object per `data: ` event. Frames are delimited
 * by a blank line (`\n\n`). Aborting the AbortSignal closes the underlying
 * connection and ends iteration.
 *
 * Scope: opencode /event SSE only. We don't implement reconnection or
 * `id:`/`event:` handling (opencode doesn't use them for our purposes).
 */

export async function* subscribeSse(
  url: string,
  signal: AbortSignal,
): AsyncGenerator<unknown, void, void> {
  // Use a separate controller for the fetch so that aborting the caller's
  // signal does not cause undici to throw synchronously from within the
  // abort event dispatch — which would surface as an unhandled rejection in
  // the test harness.  We tie the two signals together manually below.
  const fetchCtrl = new AbortController();
  const res = await fetch(url, { signal: fetchCtrl.signal });
  if (!res.ok || !res.body) {
    fetchCtrl.abort();
    throw new Error(`sse subscribe failed: status=${res.status}`);
  }
  const reader = res.body.getReader();
  const decoder = new TextDecoder("utf-8");
  let buf = "";

  // When the caller aborts, cancel the reader so reader.read() resolves
  // immediately with done=true (or throws AbortError — both are handled).
  const onAbort = () => { reader.cancel().catch(() => { /* noop */ }); };
  signal.addEventListener("abort", onAbort, { once: true });

  try {
    while (true) {
      if (signal.aborted) break;
      let value: Uint8Array | undefined;
      let done: boolean;
      try {
        ({ value, done } = await reader.read());
      } catch (err) {
        // AbortError means the signal fired — treat as clean end of stream.
        if (err instanceof Error && err.name === "AbortError") break;
        throw err;
      }
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      // Process complete frames (delimited by \n\n) in order.
      let idx: number;
      while ((idx = buf.indexOf("\n\n")) !== -1) {
        const frame = buf.slice(0, idx);
        buf = buf.slice(idx + 2);
        const data = extractDataPayload(frame);
        if (data === undefined) continue;
        try {
          yield JSON.parse(data);
        } catch {
          // skip malformed payload
        }
      }
    }
  } finally {
    signal.removeEventListener("abort", onAbort);
    try { reader.cancel(); } catch { /* noop */ }
  }
}

function extractDataPayload(frame: string): string | undefined {
  // A frame may carry multiple "data:" lines; concatenate them with \n
  // per the SSE spec. Other prefixes (id:, event:) are ignored here.
  const parts: string[] = [];
  for (const line of frame.split("\n")) {
    if (line.startsWith("data:")) {
      parts.push(line.slice(5).replace(/^ /, ""));
    }
  }
  if (parts.length === 0) return undefined;
  return parts.join("\n");
}
