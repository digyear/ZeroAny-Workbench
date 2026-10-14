import type { AgentDiscoveredSession } from './types';

export interface ForkTreeRow {
  session: AgentDiscoveredSession;
  depth: number;
}

/**
 * Reorder a flat discovered-session list into fork-tree order, mirroring the
 * Hermes Desktop sidebar: a /branch child renders directly beneath the session
 * it was forked from instead of floating at its own recency slot.
 *
 * - Input order is preserved for roots (callers sort by updatedAt desc first);
 *   each root's children follow it in their input order.
 * - A child whose fork parent is NOT in the list (filtered out, adopted, or a
 *   different project) stays top-level — indenting under an invisible parent
 *   would just look like a stray gap.
 * - Depth is capped at 4 and cycles are guarded so malformed lineage data can
 *   never drop rows or hang.
 */
export function orderForkTree(list: AgentDiscoveredSession[]): ForkTreeRow[] {
  const byExternal = new Map<string, AgentDiscoveredSession>();
  for (const s of list) {
    if (s.externalSessionId) byExternal.set(s.externalSessionId, s);
  }
  const childrenOf = new Map<string, AgentDiscoveredSession[]>();
  const roots: AgentDiscoveredSession[] = [];
  for (const s of list) {
    const pid = s.parentExternalSessionId;
    if (pid && pid !== s.externalSessionId && byExternal.has(pid)) {
      const bucket = childrenOf.get(pid);
      if (bucket) bucket.push(s);
      else childrenOf.set(pid, [s]);
    } else {
      roots.push(s);
    }
  }
  const rows: ForkTreeRow[] = [];
  const seen = new Set<string>();
  const walk = (session: AgentDiscoveredSession, depth: number) => {
    if (seen.has(session.id)) return; // cycle guard
    seen.add(session.id);
    rows.push({ session, depth: Math.min(depth, 4) });
    const kids = childrenOf.get(session.externalSessionId);
    if (kids) for (const kid of kids) walk(kid, depth + 1);
  };
  for (const root of roots) walk(root, 0);
  // Safety net: anything unplaced (shouldn't happen) still renders flat.
  for (const s of list) if (!seen.has(s.id)) rows.push({ session: s, depth: 0 });
  return rows;
}
