import type { AgentDiscoveredSession, AgentSession } from './types';

export interface ForkTreeRow<T> {
  session: T;
  depth: number;
}

/**
 * Reorder a flat session list into fork-tree order, mirroring the Hermes
 * Desktop sidebar: a child renders directly beneath the session it belongs to
 * instead of floating at its own recency slot.
 *
 * - Input order is preserved for roots (callers sort by recency first); each
 *   root's children follow it in their input order.
 * - A child whose parent is NOT in the list (filtered out, adopted away, or a
 *   different project) stays top-level — indenting under an invisible parent
 *   would just look like a stray gap.
 * - Depth is capped at 4 and cycles are guarded so malformed lineage data can
 *   never drop rows or hang.
 *
 * `parentOf` resolves each row's parent ROW (or null for top-level), so
 * callers can mix lineage sources (provider fork metadata, manual drag&drop).
 */
function buildForkTree<T>(
  list: T[],
  idOf: (s: T) => string,
  parentOf: (s: T) => T | null,
): ForkTreeRow<T>[] {
  const childrenOf = new Map<string, T[]>();
  const roots: T[] = [];
  for (const s of list) {
    const parent = parentOf(s);
    if (parent && idOf(parent) !== idOf(s)) {
      const key = idOf(parent);
      const bucket = childrenOf.get(key);
      if (bucket) bucket.push(s);
      else childrenOf.set(key, [s]);
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
    const kids = childrenOf.get(id);
    if (kids) for (const kid of kids) walk(kid, depth + 1);
  };
  for (const root of roots) walk(root, 0);
  // Safety net: anything unplaced (shouldn't happen) still renders flat.
  for (const s of list) if (!seen.has(idOf(s))) rows.push({ session: s, depth: 0 });
  return rows;
}

/** Fork-tree ordering for discovered (external) sessions — Session Center and
 *  the AgentNav DISCOVERED preview. Lineage: `parentExternalSessionId` matches
 *  the parent row's `externalSessionId` (provider-native id). */
export function orderForkTree(
  list: AgentDiscoveredSession[],
): ForkTreeRow<AgentDiscoveredSession>[] {
  const byExternal = new Map<string, AgentDiscoveredSession>();
  for (const s of list) {
    if (s.externalSessionId) byExternal.set(s.externalSessionId, s);
  }
  return buildForkTree(list, (s) => s.id, (s) => {
    const pid = s.parentExternalSessionId;
    if (!pid || pid === s.externalSessionId) return null;
    return byExternal.get(pid) ?? null;
  });
}

/** Fork-tree ordering for managed sessions — the AgentNav project groups.
 *
 *  Parent resolution priority:
 *  1. Manual drag&drop (`parentSessionId` = another row's id) — always wins;
 *  2. `parentSessionId === ''` — explicitly detached, stays top-level even if
 *     automatic fork lineage would nest it;
 *  3. Automatic fork lineage: `parentExternalSessionId` matched against the
 *     parent row's `claudeSessionId` (both provider-native resume ids). Note
 *     resume ids drift with compression/branch tips, so auto nesting is
 *     best-effort — manual moves are the reliable override. */
export function orderManagedForkTree(list: AgentSession[]): ForkTreeRow<AgentSession>[] {
  const byId = new Map<string, AgentSession>();
  for (const s of list) byId.set(s.id, s);
  const byExternal = new Map<string, AgentSession>();
  for (const s of list) {
    if (s.claudeSessionId) byExternal.set(s.claudeSessionId, s);
  }
  return buildForkTree(list, (s) => s.id, (s) => {
    const manual = s.parentSessionId;
    if (manual === '') return null; // explicitly detached
    if (manual != null) return byId.get(manual) ?? null;
    const ext = s.parentExternalSessionId;
    if (ext && ext !== s.claudeSessionId) return byExternal.get(ext) ?? null;
    return null;
  });
}

/** Rows of `list` that would render nested directly under `target` — same
 *  resolution as orderManagedForkTree. Used to block deleting a session that
 *  still has visible children. */
export function childRowsOf(list: AgentSession[], target: AgentSession): AgentSession[] {
  return list.filter((s) => {
    if (s.id === target.id) return false;
    const manual = s.parentSessionId;
    if (manual === target.id) return true;
    if (manual != null) return false; // detached ('') or manual child elsewhere
    const ext = s.parentExternalSessionId;
    return !!ext && !!target.claudeSessionId && ext === target.claudeSessionId;
  });
}
