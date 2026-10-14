import type { AgentDiscoveredSession, AgentSession } from './types';

export interface ForkTreeRow<T> {
  session: T;
  depth: number;
}

/**
 * Reorder a flat session list into fork-tree order, mirroring the Hermes
 * Desktop sidebar: a /branch child renders directly beneath the session it was
 * forked from instead of floating at its own recency slot.
 *
 * - Input order is preserved for roots (callers sort by recency first); each
 *   root's children follow it in their input order.
 * - A child whose fork parent is NOT in the list (filtered out, adopted away,
 *   or a different project) stays top-level — indenting under an invisible
 *   parent would just look like a stray gap.
 * - Depth is capped at 4 and cycles are guarded so malformed lineage data can
 *   never drop rows or hang.
 *
 * `externalId`/`parentExternalId` are provider-native ids: for discovered
 * sessions the external id is `externalSessionId`; for managed sessions it is
 * `claudeSessionId` (the resume id), matched against the child's
 * `parentExternalSessionId`.
 */
function buildForkTree<T>(
  list: T[],
  idOf: (s: T) => string,
  externalIdOf: (s: T) => string | null | undefined,
  parentExternalIdOf: (s: T) => string | null | undefined,
): ForkTreeRow<T>[] {
  const byExternal = new Map<string, T>();
  for (const s of list) {
    const ext = externalIdOf(s);
    if (ext) byExternal.set(ext, s);
  }
  const childrenOf = new Map<string, T[]>();
  const roots: T[] = [];
  for (const s of list) {
    const pid = parentExternalIdOf(s);
    if (pid && pid !== externalIdOf(s) && byExternal.has(pid)) {
      const bucket = childrenOf.get(pid);
      if (bucket) bucket.push(s);
      else childrenOf.set(pid, [s]);
    } else {
      roots.push(s);
    }
  }
  const rows: ForkTreeRow<T>[] = [];
  const seen = new Set<string>();
  const walk = (session: T, depth: number) => {
    const id = idOf(session);
    if (seen.has(id)) return; // cycle guard
    seen.add(id);
    rows.push({ session, depth: Math.min(depth, 4) });
    const ext = externalIdOf(session);
    const kids = ext ? childrenOf.get(ext) : undefined;
    if (kids) for (const kid of kids) walk(kid, depth + 1);
  };
  for (const root of roots) walk(root, 0);
  // Safety net: anything unplaced (shouldn't happen) still renders flat.
  for (const s of list) if (!seen.has(idOf(s))) rows.push({ session: s, depth: 0 });
  return rows;
}

/** Fork-tree ordering for discovered (external) sessions — Session Center and
 *  the AgentNav DISCOVERED preview. */
export function orderForkTree(
  list: AgentDiscoveredSession[],
): ForkTreeRow<AgentDiscoveredSession>[] {
  return buildForkTree(
    list,
    (s) => s.id,
    (s) => s.externalSessionId,
    (s) => s.parentExternalSessionId,
  );
}

/** Fork-tree ordering for managed sessions — the AgentNav project groups.
 *  A managed child's `parentExternalSessionId` matches its parent's
 *  `claudeSessionId` (both provider-native resume ids). */
export function orderManagedForkTree(list: AgentSession[]): ForkTreeRow<AgentSession>[] {
  return buildForkTree(
    list,
    (s) => s.id,
    (s) => s.claudeSessionId,
    (s) => s.parentExternalSessionId,
  );
}
