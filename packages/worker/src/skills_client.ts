/**
 * P2.2 worker-side skills fetcher. Single GET against the world's
 * /api/agents/:id/skills endpoint with bearer auth. The shape is the
 * one prepareWorkdir consumes — { name, content_md, files? }[] — verbatim.
 *
 * P3 carry-forward: `files` is an optional array of associated text files
 * the worker materializes into `<workdir>/skills/<skill-name>/<file>`.
 */

export interface SkillFile {
  name: string;
  content: string;
}

export interface SkillContent {
  name: string;
  content_md: string;
  files?: SkillFile[];
}

export async function fetchSkillsForAgent(
  worldHttpBase: string,
  agentId: string,
  secret: string,
): Promise<SkillContent[]> {
  const url = `${worldHttpBase.replace(/\/$/, "")}/api/agents/${encodeURIComponent(agentId)}/skills`;
  const res = await fetch(url, {
    method: "GET",
    headers: { Authorization: `Bearer ${agentId}:${secret}` },
  });
  if (!res.ok) {
    throw new Error(`fetchSkillsForAgent failed: status=${res.status}`);
  }
  const body = (await res.json()) as { skills?: SkillContent[] };
  return Array.isArray(body.skills) ? body.skills : [];
}
