# Burrata UI style guide

## Theme and layout

The desktop app defines its palette and control defaults in [style.css](../src/style.css). Use `--base` for the canvas, `--surface` for panels, `--raised` for elevated controls, and `--line` for dividers. Use `--text` for labels and `--muted` for supporting information. Pair status colors with readable text or an icon. Native form controls inherit the shell’s color scheme.

Keep component styles focused on layout. Use the inherited typeface, allow action groups to wrap, and give long titles a way to wrap or reveal their full text. Keep the page heading and its main action visible at narrow widths. Reuse TrackList, ArtistBanner and existing action menus when displaying the same content in another view.

## Buttons and navigation

| Purpose | Pattern |
| --- | --- |
| Main action in a group | Use a button with `.primary`, such as Play station or Save connections. |
| Supporting action | Use the default button for Edit, Cancel or Refresh. |
| Destructive action | Use `.danger` and name the affected item when the context is ambiguous. |
| Switch between sections | Use a named navigation group with `.section-switcher`. Use `aria-pressed` for view choices or `aria-current="page"` for the current destination. |
| Open a menu or dialog | Use a short action label. Use a trailing ellipsis when the action needs another choice. Give icon-only triggers an accessible name. |

Shared action buttons use a neutral fill and edge, with separate hover and pressed colors. The primary action uses the theme accent and a subtle raised edge. Use these shared styles directly in forms and dialogs. Keep icon controls and menu rows compact for playback and track lists.

Use a link for a URL and a native button for an action. Preserve the visible keyboard focus ring. Keep hover, focus, selection and disabled states distinct. For a tablist with tab panels, follow the keyboard behavior in [WorkspaceTabs](../src/WorkspaceTabs.svelte).

In Settings, size forms against the settings pane. Keep label and help text with the field, and stack paired fields when the pane narrows. The category menu switches to a labeled native select when the six destinations would wrap.

## Action placement

- In a page header, put the frequent named action first and the page overflow trigger last. In a track row, keep Favorite, Download and More in that order when available. Keep More at the trailing edge as titles change length.
- Use the horizontal ellipsis (⋯) for an overflow button. Give it a name that identifies its scope, such as “Library options” or “Actions for [track title]”. A trailing ellipsis on a command label means the command opens another choice. Show the overflow trigger only when the view supplies a command or destination. Place explanations next to the value or control they describe.
- Keep related menu items together. Use a divider between groups and put removal last. In confirmation dialogs, place Cancel before the named commit action. Keep DOM and visual order aligned for keyboard users. [W3C focus order guidance](https://www.w3.org/WAI/WCAG22/Understanding/focus-order.html)

## Accessibility and feedback

- Keep frequently used controls reachable with Tab and give focus a visible outline. Keep the skip link first so keyboard users can bypass the sidebar.
- Aim for at least 24 by 24 CSS pixels for standalone controls. Give touch controls more room where the layout allows it. [WCAG target size guidance](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html)
- Check text against its actual background in both themes. Aim for a contrast ratio of at least 4.5 to 1 for normal text. Include a label or shape for selection, warnings and errors so the state remains clear with system colors.
- Respect reduced motion preferences. Keep loading text visible while work is pending. For a recoverable failure, name the failed action and provide the available next step.
- Try a 320 CSS pixel viewport and increased text size. Keep supporting text near its control and give long labels room to wrap.

App-wide action feedback appears in the upper-right notification stack. Use a polite status for confirmations and an alert for errors that need attention. Keep field validation beside its field and persistent playback diagnostics with the player.

## Checking a change

Inspect the affected view in light and dark themes at desktop, compact and narrow widths. Exercise it with the keyboard, including opening and closing menus and dialogs. Check long labels, empty and populated lists, and any loading, disabled or error states the change touches. For playback and provider-backed views, use representative fixtures or a desktop session to exercise the data-dependent controls.
