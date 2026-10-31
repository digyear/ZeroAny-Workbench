import { writable, derived } from 'svelte/store';
import { STORAGE_KEYS } from '$lib/shared/constants/storage';

export type Provider = 'github' | 'google';

export interface CloudUser {
  userId: number;
  email: string | null;
  displayName: string | null;
  firstName: string | null;
  lastName: string | null;
  avatarUrl: string | null;
  slug: string;
  /** ISO timestamp when the user row was created — drives "Member since". */
  createdAt: string | null;
}

export type CloudSubscriptionSnapshot = {
  status: string;
  cancelAtPeriodEnd: boolean;
  isLifetime: boolean;
  currentPeriodEnd: string | null;
  currentPeriodStart: string | null;
  interval: 'monthly' | 'yearly' | 'lifetime' | null;
  priceUsd: number | null;
} | null;

export interface CloudProviderLink {
  provider: Provider;
  providerUserId: string;
  providerLogin: string | null;
  email: string | null;
  linkedAt: string;
  lastSeenAt: string;
}

// ─── ProState — single source of truth for Pro entitlement ──────────────────
//
// Mirrors the Rust `cloud::pro_state::ProState` struct field-for-field. Only
// updated by the `cloud:pro-state` Tauri event (subscribed in +layout.svelte)
// or by the initial `pro_state_current` invoke at boot. NEVER mutated
// directly — the Rust ProStateManager is authoritative; this is the
// projection used by all 9 Pro-gated UI components via the derived stores
// below.

export type ProStateCredits = {
  remaining: number;
  allowance: number;
  resets_at: string | null;
};

export type ProStateSubscription = {
  status: string;
  cancel_at_period_end: boolean;
  is_lifetime: boolean;
  current_period_end: string | null;
  current_period_start: string | null;
  interval: 'monthly' | 'yearly' | 'lifetime' | null;
  price_usd: number | null;
};

export type ProState = {
  plan: string;
  credits: ProStateCredits | null;
  subscription: ProStateSubscription | null;
};

export const proState = writable<ProState>({
  plan: 'free',
  credits: null,
  subscription: null,
});

// ─── Back-compat derived stores ─────────────────────────────────────────────
//
// Every Pro-gated component reads `$cloudPlan` / `$cloudCredits` / `$cloudSub`.
// Those are now READ-ONLY views over `proState` so the 9 components keep
// working unchanged. Their previous shape is preserved (camelCase) — the
// derived adapters remap from `proState`'s Rust-shaped (snake_case) fields.

export const cloudPlan = derived(proState, ($p) => $p.plan);

export type CloudCreditsSnapshot = {
  remaining: number;
  allowance: number;
  resetsAt: string | null;
} | null;

export const cloudCredits = derived<typeof proState, CloudCreditsSnapshot>(
  proState,
  ($p) =>
    $p.credits
      ? {
          remaining: $p.credits.remaining,
          allowance: $p.credits.allowance,
          resetsAt: $p.credits.resets_at,
        }
      : null,
);

export const cloudSub = derived<typeof proState, CloudSubscriptionSnapshot>(
  proState,
  ($p) =>
    $p.subscription
      ? {
          status: $p.subscription.status,
          cancelAtPeriodEnd: $p.subscription.cancel_at_period_end,
          isLifetime: $p.subscription.is_lifetime,
          currentPeriodEnd: $p.subscription.current_period_end,
          currentPeriodStart: $p.subscription.current_period_start,
          interval: $p.subscription.interval,
          priceUsd: $p.subscription.price_usd,
        }
      : null,
);

// ─── Identity + UI-only stores (NOT entitlement state) ──────────────────────

export const cloudConnected = writable<boolean>(false);
export const cloudUser = writable<CloudUser | null>(null);
export const cloudProviders = writable<CloudProviderLink[]>([]);
export const activeProvider = writable<Provider | null>(null);
export const welcomeProModalOpen = writable<boolean>(false);

/** Transient hint set by the post-checkout deep-link (`?plan=monthly|yearly|
 *  lifetime`).
 *  but if the Polar `order.paid` webhook hasn't reached our worker yet the
 *  modal opens with this URL hint so the user still sees "Pro Monthly" /
 *  "Pro Yearly" / "Pro Lifetime" — not the generic "Pro" fallback. Cleared
 *  when the modal closes. */
export const welcomeProPlanHint = writable<'monthly' | 'yearly' | 'lifetime' | null>(null);

/** True while the post-checkout deep-link handler is polling /api/auth/me for
 *  the Polar webhook to land.
 *  loading state when this is true and the celebration state once false. */
export const postCheckoutVerifying = writable<boolean>(false);

/** Convenience: the GitHub-or-Google display handle shown in UI. */
export const cloudDisplayHandle = derived(
  [cloudUser, cloudProviders, activeProvider],
  ([$u, $p, $active]) => {
    if (!$u) return null;
    const linked = $active ? $p.find((p) => p.provider === $active) : $p[0];
    return {
      handle:
        linked?.providerLogin ||
        $u.displayName ||
        $u.email ||
        $u.slug,
      avatarUrl: $u.avatarUrl,
      provider: linked?.provider || $active || null,
    };
  },
);

/** Per-domain "last synced" timestamps, keyed by kind. */
export const lastSyncedByKind = writable<Record<string, string>>({});

/** Generic "any push or pull in flight" flag for spinner states. */
export const syncing = writable<boolean>(false);

/** Kinds currently in conflict-locked state — populated by listening to
 *  the `cloud:conflicts-changed` Tauri event. */
export const cloudConflicts = writable<string[]>([]);

/** Show the "Cloud data found — restore?" modal on first sign-in when local has rows. */
export const showSyncRestorePrompt = writable<boolean>(false);

/** Show the "Set up this device" modal when both local and cloud have data
 *  on a device that has never synced — the user picks merge/keep/cloud. */
export const showDeviceSetup = writable<boolean>(false);

/** Persisted: did the user complete the first-sign-in restore decision? */
export const hasSyncedOnce = writable<boolean>(
  typeof localStorage !== 'undefined'
    ? localStorage.getItem(STORAGE_KEYS.HAS_SYNCED) === 'true'
    : false,
);

export const upgradeModalOpen = writable<boolean>(false);

export function markSynced() {
  hasSyncedOnce.set(true);
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem(STORAGE_KEYS.HAS_SYNCED, 'true');
  }
}

/**
 * Set identity-side stores (user, providers, active provider) on sign-in.
 *
 * The `plan` parameter is kept for back-compat with existing callers but
 * ignored — the plan now lives in `proState`, updated authoritatively by the
 * Rust ProStateManager via the `cloud:pro-state` Tauri event. The derived
 * `cloudPlan` store reflects whatever the manager last published.
 */
export function setConnected(
  user: CloudUser,
  providers: CloudProviderLink[],
  active: Provider | null,
  _plan_unused?: string,
) {
  cloudConnected.set(true);
  cloudUser.set(user);
  cloudProviders.set(providers);
  activeProvider.set(active);
  if (user.avatarUrl && typeof localStorage !== 'undefined') {
    localStorage.setItem(STORAGE_KEYS.GITHUB_AVATAR, user.avatarUrl);
  }
}

/**
 * Clear identity-side stores on sign-out. The proState is cleared by the
 * Rust ProStateManager (via `cloud_logout` → `manager.clear()`) which emits
 * `cloud:pro-state` with the free default — the derived stores reactively
 * follow, so cloudPlan flips to 'free' without us touching it here.
 */
export function setDisconnected() {
  cloudConnected.set(false);
  cloudUser.set(null);
  cloudProviders.set([]);
  activeProvider.set(null);
  welcomeProModalOpen.set(false);
  welcomeProPlanHint.set(null);
  postCheckoutVerifying.set(false);
  upgradeModalOpen.set(false);
  lastSyncedByKind.set({});
  hasSyncedOnce.set(false);
  showSyncRestorePrompt.set(false);
  showDeviceSetup.set(false);
  cloudConflicts.set([]);
  if (typeof localStorage !== 'undefined') {
    localStorage.removeItem(STORAGE_KEYS.GITHUB_AVATAR);
    localStorage.removeItem(STORAGE_KEYS.HAS_SYNCED);
    localStorage.removeItem(STORAGE_KEYS.LAST_SYNCED_AT);
  }
}

export function setSyncing(value: boolean) {
  syncing.set(value);
}

export function setLastSyncedForKinds(map: Record<string, string>) {
  lastSyncedByKind.set(map);
}

// Restore the cached avatar on first import so the UI doesn't flash.
if (typeof localStorage !== 'undefined') {
  const cachedAvatar = localStorage.getItem(STORAGE_KEYS.GITHUB_AVATAR);
  if (cachedAvatar) {
    cloudUser.update((u) =>
      u ? { ...u, avatarUrl: cachedAvatar } : u,
    );
  }
}
