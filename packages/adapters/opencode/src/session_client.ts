/**
 * Thin fetch wrappers around opencode serve's session API. Kept tiny and
 * dependency-free so it's trivial to swap or stub. opencode runs on
 * 127.0.0.1 with no auth (same trust model as our other 127.0.0.1
 * helpers).
 */

export interface OpencodeModel {
  providerID: string;
  modelID: string;
}

export interface CreatedSession {
  id: string;
}

export async function createSession(baseUrl: string, cwd: string): Promise<CreatedSession> {
  const res = await fetch(`${baseUrl}/session`, {
    method: "POST",
    headers: { "x-opencode-directory": cwd },
  });
  if (!res.ok) {
    throw new Error(`createSession failed: status=${res.status}`);
  }
  const body = (await res.json()) as { id: string };
  return { id: body.id };
}

export interface SendMessageOpts {
  sessionId: string;
  prompt: string;
  agent: string;
  model: OpencodeModel;
}

export async function sendMessage(baseUrl: string, opts: SendMessageOpts): Promise<void> {
  const res = await fetch(`${baseUrl}/session/${opts.sessionId}/message`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      parts: [{ type: "text", text: opts.prompt }],
      agent: opts.agent,
      model: opts.model,
    }),
  });
  if (!res.ok) {
    throw new Error(`sendMessage failed: status=${res.status}`);
  }
  // Drain the response body so the connection releases.
  await res.text();
}

export async function deleteSession(baseUrl: string, sessionId: string): Promise<void> {
  await fetch(`${baseUrl}/session/${sessionId}`, { method: "DELETE" });
}
