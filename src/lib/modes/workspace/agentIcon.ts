// One source of truth for "what does provider X look like?"  Used by:
//   • CardThread chat bubbles — round avatar to the left/right of the body
//   • BoardView in-flight indicator — small pulsing chip on the card
//   • CardEditorDrawer linked-session card — leading icon
//
// Returning an SVG string lets callers drop it inline with `{@html}` —
// avoids a per-provider Svelte component file when each is just one tag.

export interface AgentIcon {
  /** Inline SVG markup. */
  svg: string;
  /** Brand-ish colour for backgrounds/borders. */
  color: string;
  /** Display label, capitalised. */
  label: string;
}

// Claude / Anthropic brand mark — exactly the icon shown next to active
// Claude sessions in the agent nav (`static/code-no-action.svg`),
// inlined so workspace consumers (chat bubbles, card chips) render it
// without an extra HTTP fetch.
const CLAUDE =
  '<svg width="11" height="11" viewBox="0 0 24 24" fill="#D97757" fill-rule="evenodd" xmlns="http://www.w3.org/2000/svg"><path clip-rule="evenodd" d="M20.998 10.949H24v3.102h-3v3.028h-1.487V20H18v-2.921h-1.487V20H15v-2.921H9V20H7.488v-2.921H6V20H4.487v-2.921H3V14.05H0V10.95h3V5h17.998v5.949zM6 10.949h1.488V8.102H6v2.847zm10.51 0H18V8.102h-1.49v2.847z" fill="#D97757" fill-rule="evenodd"/></svg>';

const TERMINAL =
  '<svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>';

// OpenAI Codex brand mark — stylised "<" + "=" inside a rounded petal.
// Fills with `currentColor` so it picks up the surrounding text colour
// and themes correctly across dark / light surfaces.
const CODEX =
  '<svg width="11" height="11" viewBox="0 0 24 24" fill="currentColor" fill-rule="evenodd" xmlns="http://www.w3.org/2000/svg"><path clip-rule="evenodd" d="M8.086.457a6.105 6.105 0 013.046-.415c1.333.153 2.521.72 3.564 1.7a.117.117 0 00.107.029c1.408-.346 2.762-.224 4.061.366l.063.03.154.076c1.357.703 2.33 1.77 2.918 3.198.278.679.418 1.388.421 2.126a5.655 5.655 0 01-.18 1.631.167.167 0 00.04.155 5.982 5.982 0 011.578 2.891c.385 1.901-.01 3.615-1.183 5.14l-.182.22a6.063 6.063 0 01-2.934 1.851.162.162 0 00-.108.102c-.255.736-.511 1.364-.987 1.992-1.199 1.582-2.962 2.462-4.948 2.451-1.583-.008-2.986-.587-4.21-1.736a.145.145 0 00-.14-.032c-.518.167-1.04.191-1.604.185a5.924 5.924 0 01-2.595-.622 6.058 6.058 0 01-2.146-1.781c-.203-.269-.404-.522-.551-.821a7.74 7.74 0 01-.495-1.283 6.11 6.11 0 01-.017-3.064.166.166 0 00.008-.074.115.115 0 00-.037-.064 5.958 5.958 0 01-1.38-2.202 5.196 5.196 0 01-.333-1.589 6.915 6.915 0 01.188-2.132c.45-1.484 1.309-2.648 2.577-3.493.282-.188.55-.334.802-.438.286-.12.573-.22.861-.304a.129.129 0 00.087-.087A6.016 6.016 0 015.635 2.31C6.315 1.464 7.132.846 8.086.457zm-.804 7.85a.848.848 0 00-1.473.842l1.694 2.965-1.688 2.848a.849.849 0 001.46.864l1.94-3.272a.849.849 0 00.007-.854l-1.94-3.393zm5.446 6.24a.849.849 0 000 1.695h4.848a.849.849 0 000-1.696h-4.848z"/></svg>';

const ROBOT =
  '<svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="7" width="16" height="12" rx="2"/><path d="M12 7V3"/><circle cx="9" cy="13" r="1"/><circle cx="15" cy="13" r="1"/></svg>';

// OpenCode brand mark — two-tone rectangles. Using the dark-theme palette
// (light grays on dark) since the app's primary theme is dark; the SVG's
// own viewBox preserves the 4:5 aspect inside an 11x11 wrapper.
const OPENCODE =
  '<svg width="11" height="11" viewBox="0 0 240 300" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M180 240H60V120H180V240Z" fill="#4B4646"/><path d="M180 60H60V240H180V60ZM240 300H0V0H240V300Z" fill="#F1ECEC"/></svg>';

// Hermes Agent mark — a winged spark in the Nous/Hermes purple. Inline so
// workspace consumers (chat bubbles, card chips) render it without a fetch.
const HERMES =
  '<svg width="11" height="11" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M13 2L4.5 13.5H11L9.5 22L19 9.5H12.5L13 2Z" fill="#7c5cf8"/></svg>';

// Google Gemini brand mark — four-color gradient spark. Gradient ids are
// suffixed with "-cl" to namespace them away from any other Gemini SVG
// that might also be rendered on the page (prevents def-id collisions).
const GEMINI =
  '<svg width="11" height="11" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z" fill="#3186FF"/><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z" fill="url(#cl-gemini-0)"/><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z" fill="url(#cl-gemini-1)"/><path d="M20.616 10.835a14.147 14.147 0 01-4.45-3.001 14.111 14.111 0 01-3.678-6.452.503.503 0 00-.975 0 14.134 14.134 0 01-3.679 6.452 14.155 14.155 0 01-4.45 3.001c-.65.28-1.318.505-2.002.678a.502.502 0 000 .975c.684.172 1.35.397 2.002.677a14.147 14.147 0 014.45 3.001 14.112 14.112 0 013.679 6.453.502.502 0 00.975 0c.172-.685.397-1.351.677-2.003a14.145 14.145 0 013.001-4.45 14.113 14.113 0 016.453-3.678.503.503 0 000-.975 13.245 13.245 0 01-2.003-.678z" fill="url(#cl-gemini-2)"/><defs><linearGradient gradientUnits="userSpaceOnUse" id="cl-gemini-0" x1="7" x2="11" y1="15.5" y2="12"><stop stop-color="#08B962"/><stop offset="1" stop-color="#08B962" stop-opacity="0"/></linearGradient><linearGradient gradientUnits="userSpaceOnUse" id="cl-gemini-1" x1="8" x2="11.5" y1="5.5" y2="11"><stop stop-color="#F94543"/><stop offset="1" stop-color="#F94543" stop-opacity="0"/></linearGradient><linearGradient gradientUnits="userSpaceOnUse" id="cl-gemini-2" x1="3.5" x2="17.5" y1="13.5" y2="12"><stop stop-color="#FABC12"/><stop offset=".46" stop-color="#FABC12" stop-opacity="0"/></linearGradient></defs></svg>';

export function agentIcon(provider: string | null | undefined): AgentIcon {
  switch ((provider ?? '').toLowerCase()) {
    case 'claude':
      return { svg: CLAUDE, color: '#D97757', label: 'Claude' };
    case 'codex':
      return { svg: CODEX, color: '#10a37f', label: 'Codex' };
    case 'gemini':
      return { svg: GEMINI, color: '#3186ff', label: 'Gemini' };
    case 'opencode':
      return { svg: OPENCODE, color: '#f1ecec', label: 'OpenCode' };
    case 'hermes':
      return { svg: HERMES, color: '#7c5cf8', label: 'Hermes' };
    case 'aider':
      return { svg: TERMINAL, color: '#f59e0b', label: 'Aider' };
    default:
      return { svg: ROBOT, color: '#9ca3af', label: provider || 'agent' };
  }
}
