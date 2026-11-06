# @design-system/ui

Shared React component library for the Design System. One package, two ways to consume it.

## 1. In the `design-system` dashboard — direct from source (dev)

The dashboard's frontend imports this package's **TypeScript source** directly, through a Vite
alias (`@design-system/ui` → `packages/ui/src/components.ts`). No build step sits in the way: edit a component
here and the dashboard hot-reloads. This is the live playground — see `frontend/src/pages/`.

The showcase home page is `/`, with a searchable app grid and a **New chat** action.
Its live catalog (`frontend/src/lib/home-apps.ts`) combines this machine's dashboards with
apps on paired machines, refreshing every 30 seconds through the existing app relay's
fixed `GET /dashboards` and `GET /fleet/dashboards` endpoints. Each compact tile names its
machine, and filters/search cover the combined catalog. App names wrap across lines.
Local and remote catalogs load independently, retaining the last successful results when a
refresh fails. Locked machines offer **Unlock**, with a native modal for username and password,
inline API errors, pending state, Cancel/Escape, and keyboard focus restoration. Username starts
at `adi` and can be changed for machine-specific credentials. Passwords remain in the form only
until it closes; the existing ADI service verifies and remembers credentials. Unlocked machines
have **Lock** controls that forget saved credentials without unpairing them. Both actions replace
the catalog immediately and invalidate chat access caches. The same-origin relay exposes only
the existing `POST /fleet/dashboards/unlock` and `POST /fleet/dashboards/forget` actions.
Unreachable machines retain their control-panel links; apps needing service permissions open
their machine's panel rather than silently requesting a grant. Unlock does not change grants.
Stopped apps keep their wake links. Catalog reads do not start any apps.
The Design System tile opens the component gallery at `/library`. Global navigation contains
Home (the ADI logo), Chats (`/interface`), and Settings (the existing main ADI settings page).
New chat opens the chat form without resuming history or sending a message.
Keep the gallery away from `/components`: that path collides with the frontend's
`components.json` file in the Vite dev server.

### Primary ADI interface

The same React app can run as the primary interface on `app.adi`, alongside the original
settings, marketplace, and embedded-chat pages. Build it with the native API prefix:

```sh
cd frontend && bun run build:primary
```

This creates `frontend/dist-primary/` and uses native `/api/*` requests. The live dashboard
continues to use its existing `/api/app/*` relay and source hot reload. Do not point `app.adi`
at the dashboard backend: its relay forwards back to `app.adi`.

The native `adi-app` server supports `ADI_PRIMARY_UI_DIST=/path/to/dist-primary`. It serves
the React Home, Chats, gallery, and old `/chat/...` bookmarks from that build, while keeping
the original `/extended/*`, `/marketplace/*`, and `/embed/*` pages and their assets. Settings
and Fleet links stay on the primary panel's own origin. `ADI_WEBAPP_DIST` remains a separate
override for the original interface; setting it to the React build would replace Settings too.

On this machine, the control-panel LaunchAgent selects a versioned native binary and UI build
under `~/.adi/mono/deploy/design-system-primary/`; its previous plist is backed up alongside
the release. Restore that plist and reload only the control-panel service to roll back.

The browser checks also work against the primary app. Their API requests are intercepted,
including lock/unlock and chat actions:

```sh
SHOWCASE_URL=http://app.adi APP_API_BASE=/api bun frontend/tests/home-apps.browser.mjs
SHOWCASE_URL=http://app.adi APP_API_BASE=/api bun frontend/tests/app-chat.browser.mjs
SHOWCASE_URL=http://app.adi APP_API_BASE=/api bun frontend/tests/app-chat-sources.browser.mjs
bun frontend/tests/primary-app.browser.mjs
```

## 2. In other apps — built JS, bundled in (distribution)

```sh
bun install        # once
bun run build      # -> dist/
```

`bun run build` produces three things:

- `dist/index.js` + `dist/style.css` — the main ES build. Import the whole barrel (and tree-shake
  the rest) or a single file: `dist/button.js`, `dist/card.js`, `dist/theme-toggle.js`,
  `dist/question-mark.js`, `dist/help-tooltip.js`, `dist/select.js`, `dist/autocomplete.js`,
  `dist/multi-select.js`, `dist/app-layout.js`, `dist/scroll-sidebar.js`, `dist/chat.js`,
  `dist/chat-composer.js`, `dist/chat-tool-call.js`. React and the small runtime deps
  are **external** — the host app provides them. Import `@design-system/ui/style.css` alongside.
- `dist/standalone/design-system-ui.js` — ONE self-contained file with the CSS inlined and
  injected as a `<style>` tag on import. Drop it into a host app, import it once at the root, and
  every page gets the components and their styles. Only React/ReactDOM stay external.
- `dist/**/*.d.ts` — types.

The `exports` map wires these up:

```js
import { Button, Card } from "@design-system/ui";      // main barrel  (+ import the css)
import { Button } from "@design-system/ui/button";      // one file
import { QuestionMark } from "@design-system/ui/question-mark";
import { HelpTooltip } from "@design-system/ui/help-tooltip";
import { Select } from "@design-system/ui/select";
import { Autocomplete } from "@design-system/ui/autocomplete";
import { MultiSelect } from "@design-system/ui/multi-select";
import { AppLayout } from "@design-system/ui/app-layout";
import { ScrollSidebar } from "@design-system/ui/scroll-sidebar";
import { ChatThread, ChatMessage } from "@design-system/ui/chat";
import { ChatComposer } from "@design-system/ui/chat-composer";
import { ChatToolCall, ChatToolOutput, ChatToolDiff } from "@design-system/ui/chat-tool-call";
import { Button } from "@design-system/ui/standalone";  // self-contained, styles included
```

## Theme

`src/styles.css` ships the default token palette (`--primary`, `--card`, …) and a `.dark`
override, so components are styled out of the box. A host re-themes by overriding those CSS
variables. The dashboard defines the same tokens in its own `index.css`.

Use `bg-canvas` for a uniform application canvas, including navigation, and
`bg-background` for content: pale gray around white in light mode, and lighter charcoal
around near-black in dark mode. The showcase uses a uniform gray canvas on `body` across all pages.

`ThemeToggle` is a controlled switch: the host owns the theme and applies `.dark` to its root
element. It needs no provider. Storage and system-preference handling also belong to the host.

```tsx
import { useEffect, useState } from "react";
import { ThemeToggle, type Theme } from "@design-system/ui";
import "@design-system/ui/style.css";

function ThemeSettings() {
  const [theme, setTheme] = useState<Theme>("light");

  useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
  }, [theme]);

  return <ThemeToggle theme={theme} onThemeChange={setTheme} />;
}
```

For a single-component import, use `@design-system/ui/theme-toggle`.

## Application layout

`AppLayout` provides slots for the icon rail, second-level navigation, topbar, and content.
It owns their responsive placement: the primary rail is 48px wide, the secondary navigation
is 200px wide on desktop, and the topbar is 44px high. On smaller screens, secondary
navigation moves below the topbar. Apply `bg-canvas` to the host's `body` for the gray canvas.
Navigation and the topbar stay transparent; content uses `bg-background`, which is white in
light mode and near-black in dark mode. The host supplies menus, routing, theme state, and
any mobile menu disclosure.
In the separate layout, the secondary navigation slot owns its desktop spacing: a 32px
top offset and 12px inner top padding.

Use `surface="joined"` for one bounded workspace beside the icon rail, below the topbar.
The secondary navigation and content share its frame and rounded outer corners. Its sidebar
is 200px wide on desktop and uses `bg-background`, giving the dark workspace a near-black
sidebar just lighter than the black content and darker than the outer canvas. The topbar
stays outside with its original theme, aligned with the content column. The content has no
separate border or rounding. The surface fills the remaining viewport height below the
topbar, with 12px right and bottom margins (16px on desktop) and rounding on all four corners.
`surfaceProps` forwards native div props for styling that surface. The live chat follows the
page theme: `surfaceProps={{ className: "dark:bg-black" }}` keeps its content white in light
mode and black in dark mode, while
`secondaryNavigationProps={{ className: "bg-muted/50 dark:bg-background" }}` gives the sidebar
a subtle shade in each theme. Its portaled menus follow the page theme too. The default
`surface="separate"` keeps the independent panel.

```tsx
import { AppLayout } from "@design-system/ui";

<AppLayout
  primaryNavigation={<nav aria-label="Global navigation">{/* Logo and icon links */}</nav>}
  secondaryNavigation={<nav aria-label="Section navigation">{/* Section links */}</nav>}
  topbar={<h1 className="text-sm font-medium">Workspace</h1>}
  contentProps={{ id: "page-content", tabIndex: -1, className: "outline-none" }}
>
  <main>Page content</main>
</AppLayout>
```

The desktop secondary navigation can be resized from 160px to 480px by dragging its right
edge. The topbar and content follow the same width. Double-click the edge to restore the
layout’s default width, or focus it and use Left/Right arrows (16px steps), Home, or End.
Mobile navigation keeps its stacked layout. Pass `sidebarWidthStorageKey` to remember the
chosen width across reloads. The live app uses `adi.sidebar-width.v1` across its pages.
Double-clicking the resize edge also clears the saved preference. Invalid saved values
fall back to the default; numeric values stay within the 160–480px bounds. Storage
failures leave resizing usable.

The absolute expand button at the workspace’s upper-left toggles `focusMode`, hiding
only the outer icon rail and topbar. The section sidebar and content fill the window
together. The same button restores the layout.
Navigation stays mounted, retaining filters, drafts, and sidebar width. The `workspaceControls`
slot renders outside the layout flow and remains available on pages without a sidebar,
including Home.

Omit `secondaryNavigation` for a single navigation level. Set `hideSecondaryNavigation` to
hide a supplied secondary slot while keeping it mounted, preserving its state between pages.
`primaryNavigationProps`, `secondaryNavigationProps`, `topbarProps`, and `contentProps` accept
native attributes and `className` for their slot wrappers; the root accepts div props. Supply
a skip link inside `primaryNavigation` when needed, targeting the content's ID.

The layout uses the available viewport height by default. For an embedded preview, set
`className="[--app-layout-height:24rem]"` to give its height calculations a smaller reference.
The lower-level `AppShell`, `AppSidebar`, `AppTopbar`, and `AppContent` primitives remain
available for custom layouts. For a single-component import, use `@design-system/ui/app-layout`.

## Scroll sidebar

`ScrollSidebar` provides the gallery's grouped navigation, automatic selection while scrolling,
anchor links, and a collapsible menu below the `md` breakpoint. It tracks document scrolling
and uses each section's CSS `scroll-margin-top` as its selection threshold. Place it in
`AppLayout`'s `secondaryNavigation` slot, or give its container sticky positioning yourself.

```tsx
import { ScrollSidebar, type ScrollSidebarGroup } from "@design-system/ui";

const groups: readonly ScrollSidebarGroup[] = [{
  label: "Guide",
  links: [
    { id: "overview", label: "Overview" },
    { id: "details", label: "Details" },
  ],
}];

<ScrollSidebar title="Guide" groups={groups} />

<section id="overview" tabIndex={-1} className="scroll-mt-8">Overview</section>
<section id="details" tabIndex={-1} className="scroll-mt-8">Details</section>
```

Use unique section IDs and list links in document order. Set `tabIndex={-1}` on section
targets so selecting a link can move keyboard focus there. `title`, `browseLabel`, and
`navigationLabel` customize the desktop heading, mobile toggle, and navigation's accessible
name. The root accepts native aside props and `className`. Each instance has a unique menu ID.

Plain anchors work without a router, including fragment changes and direct links. For
client-side routing, forward the supplied anchor props through `renderLink` and pass the
router's location key as `navigationKey`, so the sidebar restores the fragment after the
destination page renders. Set `enabled={false}` away from that page to suspend tracking
while leaving the links available:

```tsx
const { pathname, key } = useLocation();

<ScrollSidebar
  groups={groups}
  hrefPrefix="/guide"
  enabled={pathname === "/guide"}
  navigationKey={key}
  renderLink={({ href, ...props }) => <Link to={href} {...props} />}
/>
```

Selecting a link collapses the mobile menu before positioning the section. Escape closes the
menu and returns focus to its toggle. An explicitly selected short section remains highlighted
near the end of the page until the reader scrolls. For a single-component import, use
`@design-system/ui/scroll-sidebar`.

## Question marks and help

`QuestionMark` is a plain, decorative icon. It accepts span props and `className`, with no
tooltip, click behavior, or automatic positioning. Place it wherever your layout needs it:

```tsx
import { HelpTooltip, QuestionMark } from "@design-system/ui";

<span className="inline-flex items-center gap-2">
  <QuestionMark /> Help
</span>
```

`HelpTooltip` wraps any content and adds a focusable question-mark trigger. Its explanation
opens on hover, keyboard focus, or tap. The default `position="top-right"` places the trigger
absolutely at the content's top-right corner. Other positions are `top-left`, `bottom-right`,
`bottom-left`, `right`, and `left`; `right` and `left` place it beside the content.

```tsx
<HelpTooltip content="Changes are saved to this workspace." label="About saving">
  <Button>Save changes</Button>
</HelpTooltip>

<HelpTooltip
  content="Your workspace includes 10 GB of storage."
  position="top-right"
  side="right"
  align="start"
  label="About workspace storage"
  className="flex w-full"
>
  <Card className="w-full">Workspace storage</Card>
</HelpTooltip>
```

`position` controls the question mark's location relative to the wrapped content. `side`
(`top`, `right`, `bottom`, or `left`) and `align` (`start`, `center`, or `end`) independently
control the popup's preferred placement; it can shift to stay within the viewport. `label`
names the help trigger for assistive technology, and `content` accepts any React node.
Use `highlightOnHover` to highlight the trigger and show motion-safe waves when hovering its
wrapped content. The default wrapper is a div sized to its content, including inside flex and
grid layouts. Use `className="flex w-full"` for a block that should fill its container. For
inline content, such as a button inside a paragraph, use `as="span"`. `className` and other
element props apply to the wrapper.

```tsx
<p>
  Need help?{" "}
  <HelpTooltip as="span" content="Changes are saved to this workspace." label="About saving">
    <Button>Save changes</Button>
  </HelpTooltip>
</p>
```

## Disabled actions

`Button` and `ThemeToggle` require a `disabledReason` string whenever `disabled` can be true.
The disabled control gets a focusable question-mark help button, positioned absolutely at its
top-right corner by default. Its explanation
opens on hover, keyboard focus, or tap, so it remains accessible even though the action itself
cannot be used.

Hovering the disabled action highlights its question-mark help control with a steady ring and
adds subtle waves. With reduced motion enabled, the ring appears without animation. The live
gallery's **Disabled buttons** section has an opt-in **Preview animations** checkbox to try the
waves regardless of that preference; this override is local to the gallery and is not shipped
with the library.

```tsx
<Button disabled disabledReason="Create a report before exporting it.">
  Export report
</Button>

// Optional: place the help trigger beside the button instead.
<Button
  disabled
  disabledReason="Only workspace owners can delete a workspace."
  disabledHelpPosition="right"
>
  Delete workspace
</Button>

// A dynamic boolean also requires a reason, including when it currently evaluates to false.
<Button disabled={!changesReviewed} disabledReason="Review the changes before applying them.">
  Apply changes
</Button>

// TypeScript error: a disabled action must explain why.
<Button disabled>Export report</Button>

// Enabled actions need no reason.
<Button>Save changes</Button>
```

Use `disabled` for this state; `aria-disabled` is not accepted as an alternative. Disabled
buttons cannot use `asChild`, because a custom child might still be interactive. Use a native
`Button` when an action can become disabled. Enabled buttons can still use `asChild`.

The disabled state wraps the action and its help control in a span that uses inline flex and
sizes to its content. `className`,
`style`, and `ref` still apply to the action button. `disabledHelpPosition` accepts the same
positions as `HelpTooltip` and also works on `ThemeToggle`. Put parent layout rules (for example grid
placement or a full-width row) on your own outer container when the state can change.

Reasons are trimmed, and an empty or whitespace-only reason throws when the action is disabled.
TypeScript guarantees that a string is supplied; this runtime check ensures the string contains
an explanation. The same contract applies to `ThemeToggle`:

```tsx
<ThemeToggle
  theme={theme}
  onThemeChange={setTheme}
  disabled
  disabledReason="Your workspace administrator controls the theme."
/>
```

## Select, autocomplete, and multiple selection

All three controls use `ChoiceOption` objects. Values must be unique and stable; use nonempty
values when a Select has a placeholder, because its placeholder uses the empty string.
Labels are plain text. `disabled` makes an option unavailable, and `keywords` adds local search
terms for Autocomplete and MultiSelect.

```tsx
import { useState } from "react";
import { Select, Autocomplete, MultiSelect, type ChoiceOption } from "@design-system/ui";

const options: ChoiceOption[] = [
  { value: "design", label: "Design", keywords: ["UX", "UI"] },
  { value: "engineering", label: "Engineering", keywords: ["development"] },
  { value: "operations", label: "Operations", disabled: true },
];
```

Give every control an accessible name with `<label htmlFor="…">` and `id`, or `aria-label`.
Use `aria-describedby` for help text and `aria-invalid` to convey a validation error.
All controls accept `size="sm" | "default" | "lg"`, `disabled`, and `className`.

### Native single select

Select preserves the browser's native keyboard, form submission, required validation, and reset
behavior. Its controlled value is the option's **string value**; `onValueChange` receives that
string. For an uncontrolled field, use `defaultValue` instead of `value`.

```tsx
function TeamField() {
  const [team, setTeam] = useState("");
  return (
    <div>
      <label htmlFor="team">Team</label>
      <Select
        id="team"
        name="team"
        options={options}
        value={team}
        onValueChange={setTeam}
        placeholder="Choose a team"
        required
      />
    </div>
  );
}
```

### Local autocomplete

Autocomplete searches the supplied options and selects one object. Its controlled value is
`ChoiceOption | null`; clearing emits `null`. MultiSelect uses `ChoiceOption[]` and emits the
complete updated array. Both accept `defaultValue` for uncontrolled use. Keep the controlled
value in state and update it in `onValueChange`.

```tsx
function TeamAutocomplete() {
  const [team, setTeam] = useState<ChoiceOption | null>(null);
  return (
    <div>
      <label htmlFor="team-search">Team</label>
      <Autocomplete
        id="team-search"
        name="team"
        options={options}
        value={team}
        onValueChange={setTeam}
        placeholder="Search teams"
        emptyMessage="No teams found."
      />
    </div>
  );
}
```

### Multiple selection

```tsx
function TeamsField() {
  const [teams, setTeams] = useState<ChoiceOption[]>([]);
  return (
    <div>
      <label htmlFor="teams">Teams</label>
      <MultiSelect
        id="teams"
        name="teams"
        options={options}
        value={teams}
        onValueChange={setTeams}
        placeholder="Select teams"
      />
    </div>
  );
}
```

### Asynchronous options

Pass `loadOptions` to either Autocomplete or MultiSelect to search remotely. It receives the
query and an `AbortSignal`; forward that signal to your request. Return `ChoiceOption` objects
and throw on a failed request so the control can show `errorMessage`. Remote results are
already filtered by the loader. Keep `loadOptions` stable by defining it outside the component
or using `useCallback`. `debounceMs` controls the request delay.

```tsx
async function loadTeams(query: string, { signal }: { signal: AbortSignal }) {
  const response = await fetch(`/api/teams?q=${encodeURIComponent(query)}`, { signal });
  if (!response.ok) throw new Error("Could not load teams");
  return (await response.json()) as ChoiceOption[];
}

function RemoteTeamField() {
  const [team, setTeam] = useState<ChoiceOption | null>(null);
  return (
    <Autocomplete
      aria-label="Team"
      loadOptions={loadTeams}
      value={team}
      onValueChange={setTeam}
      debounceMs={250}
      placeholder="Search teams"
      loadingMessage="Loading teams…"
      errorMessage="Teams could not be loaded."
      emptyMessage="No teams found."
    />
  );
}
```

For remote multiple selection, use the same loader with
`<MultiSelect aria-label="Teams" loadOptions={loadTeams} />`.

### Create a missing option

Creation is optional and composes with either local `options` or remote `loadOptions`, on both
Autocomplete and MultiSelect. Enable `creatable` to create an option whose label and value are
the trimmed input. For custom values or persistence, provide `onCreate`, which returns the
created `ChoiceOption` synchronously or asynchronously. Return the server's final value rather
than a temporary ID. Use `createLabel={(input) => "Create: " + input}` to customize the action.

```tsx
function LocalTagsField() {
  const [tags, setTags] = useState<ChoiceOption[]>([]);
  const [choices, setChoices] = useState<ChoiceOption[]>(options);
  return (
    <MultiSelect
      aria-label="Tags"
      options={choices}
      value={tags}
      onValueChange={setTags}
      creatable
      onCreate={(label) => {
        const option = { value: crypto.randomUUID(), label };
        setChoices((current) => [...current, option]);
        return option;
      }}
    />
  );
}

async function createTeam(label: string): Promise<ChoiceOption> {
  const response = await fetch("/api/teams", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ label }),
  });
  if (!response.ok) throw new Error("Could not create team");
  return (await response.json()) as ChoiceOption;
}

function RemoteCreatableTeamField() {
  const [team, setTeam] = useState<ChoiceOption | null>(null);
  return (
    <Autocomplete
      aria-label="Team"
      loadOptions={loadTeams}
      value={team}
      onValueChange={setTeam}
      creatable
      onCreate={createTeam}
    />
  );
}
```

Autocomplete and MultiSelect support Arrow Up/Down, Enter, and Escape; Backspace in an empty
MultiSelect search removes the last enabled selection. Clear controls are enabled by default;
set `clearable={false}` to omit them, and use `clearLabel` or `retryLabel` to customize the
corresponding button labels. Their `name` prop submits selected option values as hidden form
fields; MultiSelect uses repeated fields with the same name. Both accept `form` and `required`.
For these two controls, `className` styles the wrapper and `ref` points to the search input.

## Chat

`ChatThread` is a scrollable conversation with automatic following and a **Jump to latest**
button. Give it a bounded height, directly or through a flex parent. Its `direction` has two values:

- `"top-to-bottom"` (default): oldest messages first, with new replies at the bottom.
- `"bottom-to-top"`: newest messages first, with new replies at the top.

Always supply direct message children in chronological order (oldest first). The thread reverses
their rendered DOM order for `"bottom-to-top"`, so visual, keyboard, and screen-reader order agree.
It follows new content while the reader is within 64px of the latest edge. Scrolling away
preserves the message being read, including when reverse-mode replies are prepended or stream.
The jump button points toward the latest edge. Changing direction returns to the latest reply.
`autoScroll={false}` disables following on mount and content updates; an explicit direction
change still navigates to its latest edge. The scrollable log
is keyboard focusable; native props and `ref` target the outer wrapper. `label` names the log,
and `scrollToLatestLabel` customizes its jump button.

`ChatMessage` requires `from="user" | "assistant" | "system"`. Compose an optional avatar,
speaker/time metadata, content, and actions using the corresponding primitives. Always show
the speaker in `ChatMessageMeta` or provide an accessible name. Content accepts React nodes;
plain text preserves newlines and wraps long words. The host supplies any Markdown renderer,
attachments, message state, and action handlers.
Messages use at most 90% of the available width on desktop and mobile. Assistant messages
align left and user messages align right; put tool activity inside the assistant message
to preserve the same width limit.
Assistant content has no bubble background, corner rounding, or padding.
User content uses a rounded, padded `bg-muted` bubble with `text-foreground` in both themes.

```tsx
<div className="flex h-[32rem] flex-col rounded-xl border">
  <ChatThread label="Support conversation">
    <ChatMessage from="assistant">
      <div className="min-w-0">
        <ChatMessageMeta>Assistant</ChatMessageMeta>
        <ChatMessageContent>How can I help?</ChatMessageContent>
      </div>
    </ChatMessage>
  </ChatThread>
  <ChatComposer value={draft} onValueChange={setDraft} onSend={sendMessage} />
</div>
```

To show the newest messages first, use `<ChatThread direction="bottom-to-top">` with the same
chronological children. `ChatThreadDirection` exports the direction type. The showcase provides
a direction selector; its initial selection is `"top-to-bottom"`. When composing a full chat,
place `ChatComposer` below the thread for `"top-to-bottom"` and above it for `"bottom-to-top"`.
Use keyed siblings when switching their DOM order so draft and send state are preserved.

`ChatComposer` uses a controlled `value` and `onValueChange`. `onSend` receives trimmed text;
resolve its optional promise when the message is accepted, or reject it to show `errorMessage`
and retain the draft. While a send is pending, another send is blocked but typing is allowed.
The field clears as sending starts. A rejected send restores the submitted draft when
the field is still empty, preserving any new text typed during sending. Enter sends;
Shift+Enter inserts a newline. IME composition and modified shortcuts do not submit.
Set `sendOnEnter={false}` for button-only submission. `textareaProps` forwards input attributes
such as `id`, `name`, `maxLength`, and `onKeyDown`; calling `preventDefault()` in that handler
suppresses the Enter shortcut. `inputLabel`, `placeholder`, `sendLabel`, and `stopLabel` customize
copy. Optional children provide a toolbar beside the keyboard hint.

Set `isGenerating` and `onStop` to replace Send with Stop while receiving a response. The host
owns cancellation and must reset `isGenerating` after stopping. Without `onStop`, the stop
action explains that the response cannot be stopped. A disabled composer requires a nonempty
`disabledReason`, as with `Button`.

`ChatTypingIndicator` announces its `label` once as a status and animates its decorative dots
only when motion is allowed. The thread defaults to a polite live log. For streaming updates,
use `aria-live="off"` and a separate host status announcement for response start/completion to
avoid announcing every partial word; the showcase follows this pattern.

### Tool calls

`ChatToolCall` displays an operation as a quiet, single-line disclosure inside an assistant
message or on its own. Its `kind` selects a `"terminal"`, `"git"`, `"edit"`, `"agent"`, or generic `"tool"`
icon. Set `status` to `"pending"`, `"running"`, `"completed"` (default), `"error"`, or `"cancelled"`.
Active and unsuccessful states use a status icon; state labels remain available to screen
readers and are customizable through `statusLabels`. The running icon respects reduced motion.
Long titles truncate visually, keeping their full accessible text and hover tooltip.
Use the tool's name for its title, such as `exec_command` or `apply_patch`; put the command,
arguments, and affected files in expanded details. A group can list the distinct tool names
in execution order, such as `8 tool calls · exec_command, apply_patch`.
For grouped activity, pass `group={{ errorCount: 1 }}` to keep the summary neutral and show
only `1 error` in red. Leave the count absent when it is unknown; an errored group then shows
an `Error` label. Individual failed tools retain their red title and icon.

```tsx
<ChatToolCall title="exec_command" description="git status --short" kind="git" status="completed" meta="Exit 0">
  <ChatToolOutput label="Git output">{" M src/chat.tsx"}</ChatToolOutput>
</ChatToolCall>

<ChatToolCall title="apply_patch" description="Update the chat" kind="edit" status="completed" defaultOpen>
  <ChatToolDiff file="src/chat.tsx" lines={[
    { type: "deletion", content: "const tools = false;", oldLineNumber: 8 },
    { type: "addition", content: "const tools = true;", newLineNumber: 8 },
  ]} />
</ChatToolCall>
```

Children appear in a keyboard accessible disclosure. Rows start collapsed unless `defaultOpen`
is set; use `open` and `onOpenChange` for controlled disclosure state. Status updates preserve
the reader's expanded/collapsed choice. Optional `description` and `meta` appear with the
expanded details, for a working directory, exit code, duration, or file count. Without details,
the component is a plain status row.

`ChatToolOutput` preserves plain command output in a bounded, scrollable region. `ChatToolDiff`
accepts a file path and explicit context/addition/deletion lines with optional old/new line
numbers; it derives the change counts and shows both signs and color. Strings render as text,
including HTML or shell syntax. The host supplies operation state, output, and parsed diff
lines. These components display tool activity; they do not execute commands.

Use `kind="agent"` when an agent delegates work to another agent. Give the row a short title
with the agent's name and assignment, and use `status` for that agent's progress. Set `href`
to its conversation: the name becomes a link, with a separate chevron for task details and
results. `linkProps` forwards native anchor attributes, including `onClick` for host routing.

```tsx
<ChatToolCall
  kind="agent"
  title="Layout reviewer · Check chat layout"
  status="running"
  href="/chats/layout-reviewer"
>
  <p className="p-3 text-xs">Review chat spacing and keyboard navigation.</p>
</ChatToolCall>
```

`ChatComposer` accepts controlled `attachments` (`id`, `name`, optional `url` and `error`),
`onFilesSelect`, `onAttachmentRemove`, and `attachmentsBusy`. Supplying `onFilesSelect`
adds a paperclip, image paste, and file drop support. The host uploads files and owns their
preview URLs. Images can be sent without text; uploading or failed images block submission.

`ChatWaitingStatus` accepts `items` (`id` and a readable `label`) and shows the pending task
count and a truncated reason on a single line. Expand it to see all reasons. It starts
collapsed and renders nothing when the list is empty. The live Interface pins
it above the composer, uses await notes with server summaries as a fallback, and keeps raw
check commands out of the status. It stays visible even when the agent is not running.

`ChatQueuedMessage` renders a compact expandable row with a `text` preview, optional `label`
(default: “Queued”), and optional attachment children. The live Interface keeps these rows
above its composer until the server marks their turns as sent, then shows the ordinary messages.

The live Interface uploads files including PDFs (up to 25 MB each) and PNG, JPEG, WebP, and GIF images (up to 5 MB each), with six attachments per message
to the conversation’s own machine. Image drafts survive switching conversations and failed
sends. The same-origin relay forwards only the existing attachment upload and read endpoints.

The live Interface has a microphone beside the image attachment control. It reuses the app’s
`GET /api/voice` engines and `POST /api/voice/transcribe` service, including browser recognition.
Dictation appends editable text to the draft and never sends it. The engine follows the original
`adi.voice.engine.v1` browser preference, then the service's ready default, without a selector
in the composer. Recording stops and tracks are released on chat navigation. Microphone access
requires HTTPS; the HTTP showcase shows only the disabled microphone and its existing help control.

Right-click a sidebar chat (or press Shift+F10 while focused) to rename, star/unstar, or hide
it without opening it. Hide persists on the owning machine and keeps the transcript and running session intact. Rename supports Enter to save, Escape to cancel, and inline errors.

Use the star beside an open chat’s title to keep or unstar that conversation. Starred chats
show a filled star in the sidebar and retain their existing priority in the list. Changes are
saved on the owning machine through the existing run star endpoint.

Use the copy icon beside an open chat’s title to copy its thread/run ID. The tooltip shows
the full ID, and a checkmark confirms copying. HTTP hosts use a clipboard fallback.

Use the pencil beside an open chat’s title to rename it. Enter saves, Escape cancels, and
an empty name restores the generated title. Changes use the existing run rename endpoint on
the conversation’s owning machine and update both the header and sidebar.

## Browser checks

The dashboard's `/interface` is a live ADI chat client. It uses the same agents, run history,
transcript, reply, question, and stop endpoints as the current chat. A small, fixed allowlist
under `/api/app` relays to `http://app.adi/api`, preserving the app's browser origin checks.
There is no separate conversation store. Run history and messages stay in ADI; drafts stay in
the browser page while switching conversations. Agent launch receipts link to their actual
conversation when the output contains an agent name and run ID.

The sidebar's filter menu offers all sessions, starred agents, sessions started by any user,
and sessions started by the requesting user (the default). Questions needing an answer and
the open conversation remain visible. Filtering happens on each source before pagination;
the client also checks the source's launch identity for compatibility with older nodes.
The same menu groups sessions by machine, project, or no headings. Project pages load more
from their owning source and project, using the API's project labels.

The sources menu merges this machine with any selected paired nodes from the app's fleet.
Locked nodes must be unlocked in Fleet before adding them. The selection keeps at least one
source and remembers sources and grouping across reloads; the filter resets to "started by
me". Remote chats keep their node in links, query caches, drafts, and every chat action.
New chats can choose a destination from the selected sources. Fleet credentials and pairing
remain managed by the main app; the relay accepts only the existing chat paths on paired nodes.

User and assistant messages render Markdown, including GitHub-style tables and task lists.
Code blocks retain whitespace and scroll within the message; tool arguments and execution
output remain literal text.
Activity summaries show tool names rather than shell paths, command text, or argument previews;
expand a row to inspect its full arguments and output.
Assistant replies and activity share the 90% message width limit. Grouped activity derives
error counts from the API's existing failure statistics, then uses the full details once
expanded. If the API's capped statistics cannot establish an exact count, it stays unknown
until those details load. Expanding a group also distinguishes tool calls from thinking
blocks in its call count.

Question forms accept text alone, option selections, or selections with additional text.
At least one nonempty answer enables sending; other questions may be left unanswered.
Replies retain question order, including empty strings for skipped questions. Clicking a
selected single-choice option again clears it.

The interface polls active conversations every second and idle conversations every five
seconds. Folded activity loads only when expanded. The browser integration checks replace
all app API requests with fixtures, so send/stop/question checks never modify real chats:

```sh
bun frontend/tests/app-chat.browser.mjs
bun frontend/tests/app-chat-sources.browser.mjs
bun frontend/tests/home-apps.browser.mjs
bun test frontend/src/lib/home-apps.test.ts
cd frontend && bun run test:chat
```

With the showcase running, `bun run test:browser` launches an isolated headless Chrome profile
and checks the live gallery plus interactive fixtures: keyboard selection, async cancellation,
creation, error recovery, disabled options, form behavior, chat submission/IME/draft retention,
history scrolling, tool disclosure/status updates/diffs, and light/dark theme tokens. It needs
no extra test dependencies. Override `SHOWCASE_URL` and `CHROME_BIN` if your URL or Chrome path
differs from the dashboard defaults. The temporary browser profile is removed afterward.

## Adding a component

1. `src/components/<name>.tsx`
2. re-export it from `src/components.ts`
3. (optional) add it as its own entry in `vite.config.ts` to get a separate `dist/<name>.js`

`ChatComposer` immediately replaces its keyboard hint with a spinner and “Sending message…”
while `onSend` is pending. The live status is announced to assistive technology; `sendingLabel`
customizes it. The field clears immediately; a failed send restores the draft when the field is still empty.
The live chat checks the transcript after a reply error to recognize messages already accepted
by the server, without automatically repeating the send.

The new-chat page puts agent selection in the center, with quick choices for starred agents
from every selected source. Its machine-specific picker includes all agents, with starred agents first,
preserving access to personally unstarred agents and unstarred deep links. New default selections prefer starred,
chat-capable agents; read-only and unavailable agents remain disabled.

Successful new-chat launches save the chosen agent and owning machine in browser-local
`adi.last-launched-agent.v1`. New chat restores that choice when available in selected
sources. Viewing history, selecting an agent without launching, failed launches, and other
people’s sessions never update it. An unavailable saved agent falls back to an eligible
starred/default agent on a selected source.

`ChatComposer.onSendAsap` adds a lightning action using the same draft, attachment, pending,
and failure handling as normal send. The live chat exposes it while running and sends
`mode: "asap"` for delivery at the next runtime injection point, including between tool rounds.
Regular send continues to queue until the current response finishes. Existing queued messages
cannot be promoted with the current runtime API.

Right-click a starred agent in New chat (or press Shift+F10) for **Unstar for me**.
Personal overrides persist in browser-local `adi.personal-overrides.v1`, keyed by owning
machine, entity kind, and name. The client shallowly overlays fields onto shared metadata
without modifying API data or sending mutations. Other browsers and users keep the shared
values. “Unstarred for me” offers **Restore star for me**, which removes the override and
resumes inheritance. The agent picker still includes unstarred agents. Effective agent
stars also control the client’s Only starred session filter. Additional personal fields
can use the same override store; nested metadata is replaced as a whole field.
