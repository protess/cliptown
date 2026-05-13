/**
 * P2.2 worker-side skills fetcher. Single GET against the world's
 * /api/agents/:id/skills endpoint with bearer auth. The shape is the
 * one prepareWorkdir consumes — { name, content_md }[] — verbatim.
 */

export interface SkillContent {
  name: string;
  content_md: string;
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
