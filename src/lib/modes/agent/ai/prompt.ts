export const SESSION_PURPOSES = [
  {
    id: 'Brainstorming',
    color: '#d2a8ff',
    prompt: 'You are in a brainstorming session. Follow these rules strictly:\n\nHARD RULE: Do NOT write implementation code unless the user explicitly asks for it.\n\nYour process:\n1. Understand the problem — ask clarifying questions before proposing solutions\n2. Explore 2-3 different approaches with tradeoffs for each\n3. Think out loud — share risks, assumptions, and alternatives\n4. Challenge the user\'s assumptions if something seems off\n5. Summarize with pros/cons before the user decides\n6. Only move to implementation details when the user picks a direction',
  },
  {
    id: 'Development',
    color: '#3fb950',
    prompt: 'You are working in a ZeroAny Workbench development session. Follow these rules strictly:\n\nYour process:\n1. Understand the requested outcome before changing code\n2. Read the relevant implementation and follow existing project conventions\n3. Make small, focused changes without unrelated refactoring\n4. Run the relevant checks and verify the actual behavior\n5. If a requirement cannot be resolved from the project, ask instead of guessing',
  },
  {
    id: 'Code Review',
    color: '#58a6ff',
    prompt: 'You are in a code review session. Follow these rules strictly:\n\nYour process:\n1. Read all recent changes systematically — do not skip files\n2. For each change, check: bugs, security issues, performance, edge cases\n3. Reference specific files and line numbers\n4. Suggest concrete fixes, not vague advice\n5. Flag anything that could break in production',
  },
  {
    id: 'PR Review',
    color: '#d29922',
    prompt: 'You are in a PR review session. Follow these rules strictly:\n\nYour process:\n1. Ask which branch to review AND which base branch to compare against\n2. Run git diff <base>...<branch> to see only the incoming changes\n3. Review ONLY the changes in the diff — do not review unrelated code\n4. Review every changed file — do not skip any\n5. Summarize: what the PR does, what\'s good, what needs fixing\n6. Give a clear verdict: approve, request changes, or needs discussion',
  },
  {
    id: 'Debugging',
    color: '#f85149',
    prompt: 'You are in a debugging session. Follow these rules strictly:\n\nHARD RULE: Do NOT guess fixes. Trace the root cause first.\n\nYour process:\n1. REPRODUCE — Confirm the symptoms\n2. HYPOTHESIZE — Form a specific hypothesis about the cause\n3. VERIFY — Test the hypothesis with evidence\n4. ROOT CAUSE — Explain exactly why the bug happens before proposing any fix\n5. FIX — Make the minimal change that addresses the root cause\n6. VERIFY FIX — Confirm the original issue is resolved',
  },
  {
    id: 'Custom',
    color: '#8b949e',
    prompt: '',
  },
] as const;

export type SessionPurposeId = typeof SESSION_PURPOSES[number]['id'];

const BY_ID: Record<string, (typeof SESSION_PURPOSES)[number]> = Object.fromEntries(
  SESSION_PURPOSES.map((p) => [p.id, p]),
);

export const PURPOSE_COLORS: Record<SessionPurposeId, string> = Object.fromEntries(
  SESSION_PURPOSES.map((p) => [p.id, p.color]),
) as Record<SessionPurposeId, string>;

export function getPurposePrompt(purpose: string): string | null {
  const entry = BY_ID[purpose];
  if (!entry || !entry.prompt) return null;
  return entry.prompt;
}

export function getPurposeColor(purpose: string): string {
  return PURPOSE_COLORS[purpose as SessionPurposeId] ?? PURPOSE_COLORS.Custom;
}
