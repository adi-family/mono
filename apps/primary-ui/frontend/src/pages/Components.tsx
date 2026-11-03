// The component gallery for @design-system/ui, imported directly from the
// library's TypeScript source (see vite.config.ts's alias) so edits hot-reload.

import { useEffect, useState } from "react";
import { Plus } from "lucide-react";
import { useOutletContext } from "react-router-dom";

import {
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  HelpTooltip,
  QuestionMark,
  ThemeToggle,
} from "@design-system/ui";

import type { useTheme } from "../lib/theme";
import { AppLayoutExample } from "../components/AppLayoutExample";
import { ChoiceExamples } from "../components/ChoiceExamples";
import { ChatExamples } from "../components/ChatExamples";
import { CodeExample } from "../components/CodeExample";
import { ScrollSidebarExample } from "../components/ScrollSidebarExample";

const variants = ["default", "secondary", "outline", "ghost", "link", "destructive"] as const;
const sizes = ["sm", "default", "lg"] as const;
const helpPositions = ["top-right", "right", "top-left", "left", "bottom-right", "bottom-left"] as const;

const buttonCode = `<Button>Default</Button>
<Button variant="secondary">Secondary</Button>
<Button variant="outline">Outline</Button>
<Button variant="ghost">Ghost</Button>
<Button variant="link">Link</Button>
<Button variant="destructive">Destructive</Button>

<Button size="sm">Small</Button>
<Button size="default">Default size</Button>
<Button size="lg">Large</Button>
<Button size="icon" variant="outline" aria-label="Add item">
  <Plus className="size-4" aria-hidden="true" />
</Button>`;

const helpCode = `<HelpTooltip
  content="Your workspace includes 10 GB of storage."
  label="About workspace storage"
  position="top-right"
  side="right"
>
  <p>Workspace storage: 2.4 GB used</p>
</HelpTooltip>

<QuestionMark />`;

const disabledCode = `<Button
  disabled
  disabledReason="Create a report before exporting it."
>
  Export report
</Button>

<Button
  disabled={!reviewed}
  disabledReason="Review the changes before applying them."
  onClick={applyChanges}
>
  Apply changes
</Button>`;

const themeCode = `<ThemeToggle theme={theme} onThemeChange={setTheme} />`;

const cardCode = `<Card>
  <CardHeader>
    <CardTitle>Workspace storage</CardTitle>
    <CardDescription>Your current usage</CardDescription>
  </CardHeader>
  <CardContent>2.4 GB of 10 GB used</CardContent>
</Card>`;

export function Components() {
  const { theme, setTheme } = useOutletContext<ReturnType<typeof useTheme>>();
  const [changesReviewed, setChangesReviewed] = useState(false);
  const [changesApplied, setChangesApplied] = useState(false);
  const [previewAnimations, setPreviewAnimations] = useState(false);
  const [helpPosition, setHelpPosition] = useState<(typeof helpPositions)[number]>("top-right");

  useEffect(() => {
    document.title = "Design System";
  }, []);

  return (
    <main id="gallery" tabIndex={-1} className="flex min-w-0 flex-col gap-8 scroll-mt-36 outline-none md:scroll-mt-8">
      <header>
        <p className="text-muted-foreground mb-1 text-sm">@design-system/ui</p>
        <p className="text-muted-foreground text-sm">
          Live gallery of the component library. Edit a component in{" "}
          <code className="font-mono">packages/ui/src</code> and this page hot-reloads.
        </p>
      </header>

      <ChatExamples />

      <ChoiceExamples />

      <AppLayoutExample />

      <ScrollSidebarExample />

      <Card id="button-example" tabIndex={-1} className="scroll-mt-36 md:scroll-mt-8">
        <CardHeader>
          <CardTitle>Button</CardTitle>
          <CardDescription>Variants and sizes</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <div className="flex flex-wrap gap-3">
            {variants.map((v) => (
              <Button key={v} variant={v}>
                {v}
              </Button>
            ))}
          </div>
          <div className="flex flex-wrap items-center gap-3">
            {sizes.map((s) => (
              <Button key={s} size={s}>
                size {s}
              </Button>
            ))}
            <Button size="icon" variant="outline" aria-label="Add item">
              <Plus className="size-4" aria-hidden="true" />
            </Button>
          </div>
          <CodeExample code={buttonCode} label="Button" />
        </CardContent>
      </Card>

      <Card id="help-components" tabIndex={-1} className="scroll-mt-36 md:scroll-mt-8">
        <CardHeader>
          <CardTitle>Question mark &amp; help</CardTitle>
          <CardDescription>
            Add help to a button or any block. The question mark sits at the top-right corner by
            default; choose another position below.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-6">
          <div className="flex flex-wrap items-center gap-3">
            <label htmlFor="help-position" className="text-sm font-medium">
              Question mark position
            </label>
            <select
              id="help-position"
              value={helpPosition}
              onChange={(event) => setHelpPosition(event.target.value as (typeof helpPositions)[number])}
              className="border-input bg-background focus-visible:border-ring focus-visible:ring-ring/50 h-9 rounded-md border px-3 text-sm outline-none focus-visible:ring-[3px]"
            >
              {helpPositions.map((position) => (
                <option key={position} value={position}>
                  {position.replaceAll("-", " ")}
                </option>
              ))}
            </select>
          </div>
          <div className="flex flex-wrap items-center gap-8 px-3 py-2">
            <Button
              disabled
              disabledReason="Add a team member before sending an invitation."
              disabledHelpPosition={helpPosition}
            >
              Send invitation
            </Button>
            <HelpTooltip
              content="Your workspace includes 10 GB of storage. You can add more in workspace settings."
              position={helpPosition}
              side="right"
              label="About workspace storage"
              className="flex w-full sm:max-w-xs"
            >
              <div className="bg-muted/30 w-full rounded-lg border p-4">
                <p className="text-sm font-medium">Workspace storage</p>
                <p className="text-muted-foreground mt-1 text-sm">2.4 GB of 10 GB used</p>
              </div>
            </HelpTooltip>
          </div>
          <p className="text-muted-foreground text-sm">
            The help popup has its own placement: the storage example opens to the right when
            space allows. Hover, focus, or tap either question mark to try it.
          </p>
          <div className="flex items-center gap-3 border-t pt-4">
            <QuestionMark />
            <div>
              <p className="text-sm font-medium">Standalone question mark</p>
              <p className="text-muted-foreground mt-1 text-sm">
                A plain icon you can place anywhere, without a tooltip or automatic positioning.
              </p>
            </div>
          </div>
          <CodeExample code={helpCode} label="Question mark and help" />
        </CardContent>
      </Card>

      <Card id="disabled-buttons" tabIndex={-1} className="scroll-mt-36 md:scroll-mt-8" data-preview-animations={previewAnimations}>
        <CardHeader>
          <CardTitle>Disabled buttons</CardTitle>
          <CardDescription>
            Hover Export report to highlight the ? at its top-right corner. Waves pulse around the icon when
            motion is allowed; with Reduce Motion enabled, you get a steady ring instead.
            Hover, focus, or tap the ? to learn why the action is disabled.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-6">
          <div className="bg-muted/30 rounded-lg border p-4">
            <label className="flex cursor-pointer items-center gap-2 text-sm font-medium">
              <input
                id="preview-animations"
                type="checkbox"
                checked={previewAnimations}
                onChange={(event) => setPreviewAnimations(event.target.checked)}
                aria-describedby="preview-animations-description"
                className="accent-primary focus-visible:outline-ring size-4 cursor-pointer rounded focus-visible:outline-2 focus-visible:outline-offset-2"
              />
              Preview animations
            </label>
            <p id="preview-animations-description" className="text-muted-foreground mt-2 text-sm">
              Play waves on hover in this demo even with Reduce Motion enabled. Check this, then
              hover Export report below. Uncheck to follow your system preference again.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-4">
            <Button disabled disabledReason="Create a report before exporting it.">
              Export report
            </Button>
            <Button
              variant="destructive"
              disabled
              disabledReason="Only the workspace owner can delete this workspace."
            >
              Delete workspace
            </Button>
            <ThemeToggle
              theme={theme}
              onThemeChange={setTheme}
              disabled
              disabledReason="Theme changes are locked for this preview. Use the switch in the page header."
            />
          </div>
          <div className="bg-muted/30 flex flex-col gap-4 rounded-lg border p-4">
            <div>
              <p className="text-sm font-medium">Try enabling an action</p>
              <p className="text-muted-foreground mt-1 text-sm">
                Review the changes to make the button available.
              </p>
            </div>
            <label className="flex cursor-pointer items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={changesReviewed}
                onChange={(event) => {
                  setChangesReviewed(event.target.checked);
                  setChangesApplied(false);
                }}
                className="accent-primary focus-visible:outline-ring size-4 cursor-pointer rounded focus-visible:outline-2 focus-visible:outline-offset-2"
              />
              I have reviewed the changes
            </label>
            <div className="flex flex-wrap items-center gap-3">
              <Button
                disabled={!changesReviewed}
                disabledReason="Review the changes using the checkbox above before applying them."
                onClick={() => setChangesApplied(true)}
              >
                Apply changes
              </Button>
              <p className="text-muted-foreground text-sm" role="status">
                {changesApplied ? "Example changes applied." : ""}
              </p>
            </div>
          </div>
          <CodeExample code={disabledCode} label="Disabled buttons" />
        </CardContent>
      </Card>

      <Card id="theme-toggle-example" tabIndex={-1} className="scroll-mt-36 md:scroll-mt-8">
        <CardHeader>
          <CardTitle>Theme toggle</CardTitle>
          <CardDescription>A controlled switch. The host applies the selected theme to the document.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <div className="flex flex-wrap items-center gap-3">
            <ThemeToggle theme={theme} onThemeChange={setTheme} />
            <span className="text-muted-foreground text-sm">Current theme: {theme}</span>
          </div>
          <CodeExample code={themeCode} label="Theme toggle" />
        </CardContent>
      </Card>

      <Card id="card-example" tabIndex={-1} className="scroll-mt-36 md:scroll-mt-8">
        <CardHeader>
          <CardTitle>Card</CardTitle>
          <CardDescription>This panel is itself a Card from the library.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <p className="text-muted-foreground text-sm">
            Cards compose a header (title + description) and content. Add a component by dropping
            <code className="break-all font-mono"> packages/ui/src/components/&lt;name&gt;.tsx</code>, exporting
            it from <code className="font-mono">components.ts</code>, then adding it here.
          </p>
          <CodeExample code={cardCode} label="Card" />
        </CardContent>
      </Card>
    </main>
  );
}
