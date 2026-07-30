# Semantic Engine — Design system

## Intent

The desktop console should feel like a compact broadcast control surface: calm,
legible under pressure, technical without looking like a developer tool. It is
used beside a live production, often on a second monitor.

## Visual direction

- World: cue sheet, signal monitor, local equipment status.
- Background: near-black olive rather than pure black.
- Signal color: acid lime for active/accepted states.
- Warning colors: amber for abstention, muted coral for rejection.
- Geometry: compact panels, one-pixel dividers, restrained four-pixel radii.
- Type: Aptos/Segoe UI for reading; Cascadia Code/Consolas for IDs and telemetry.
- Density: operational, but every control keeps a clear label and focus state.

## Tokens

| Role | Value |
|---|---|
| canvas | `#11130f` |
| panel | `#1a1d18` |
| raised panel | `#20241e` |
| primary text | `#e9e9e1` |
| muted text | `#92988c` |
| divider | `#30342d` |
| accepted / action | `#c8f15b` |
| abstained | `#f1bd5b` |
| rejected | `#ef796f` |

## Interaction rules

- Never communicate a decision by color alone; pair icon, label and percentage.
- Enter validates when focus is in the response field.
- Buttons and fields must expose visible `:focus-visible` states.
- Reduced-motion preference removes transitions.
- History is ephemeral and limited to eight entries in the client.
- The client never labels someone as winner; it only shows engine output.

## Responsive rules

- Three operational columns above 980 px.
- Two columns with a full-width result below 980 px.
- Single linear workflow below 700 px.
- Secondary telemetry collapses before primary decision information.

## Product copy

Use “acceptée”, “à arbitrer” and “rejetée”. Avoid “l’IA pense” or claims of
understanding. Explain that the engine validates against configured context.
