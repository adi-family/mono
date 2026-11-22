import { useState } from "react";
import { ArrowUpRight, Bell, Zap } from "lucide-react";

import {
  Badge, Button, Card, CardContent, CardDescription, CardHeader, CardTitle, CodeEditor, Field, Input, SettingsLayout, SettingsNavItem,
  SettingsSection, Switch, Textarea, cn, type CodeLanguage,
} from "@design-system/ui";

import { CodeExample } from "./CodeExample";

const badgeCode = `<Badge>Webhook</Badge>
<Badge variant="outline">sh</Badge>
<Badge variant="default">New</Badge>

<Badge variant="success">Running</Badge>
<Badge variant="warning">Stopped</Badge>
<Badge variant="destructive">Failed</Badge>
<Badge variant="muted">Disabled</Badge>`;

const fieldCode = `<Field label="Name" hint="Letters, digits, '.', '-' or '_'.">
  {(control) => <Input {...control} mono placeholder="deploy-hook" />}
</Field>

<Field label="Interval" error="Enter a whole number of seconds.">
  {(control) => <Input {...control} mono value="soon" readOnly />}
</Field>

<Field label="Code block" hint="Run as sh -c.">
  {(control) => <Textarea {...control} mono rows={4} />}
</Field>`;

const switchCode = `<Switch
  aria-label="Notifications enabled"
  checked={enabled}
  onCheckedChange={setEnabled}
/>`;

const editorCode = `<Field label="Code block" hint="Run as sh -c.">
  {(control) => (
    <CodeEditor
      {...control}
      language="sh"
      rows={10}
      value={code}
      onValueChange={setCode}
    />
  )}
</Field>`;

const editorSamples: Record<Exclude<CodeLanguage, "plain">, string> = {
  sh: `#!/bin/sh
# Poll a URL and report when it changes.
previous=""
while true; do
  body=$(curl -fsS "\${ADI_URL:-https://example.test/status}")
  if [ "$body" != "$previous" ]; then
    echo "changed at $(date -u +%H:%M:%S)"
    previous="$body"
  fi
  sleep "\${ADI_INTERVAL_SECS:-300}" # five minutes by default
done
`,
  ts: `// Handle a webhook delivery.
const raw = await Bun.file(process.env.ADI_PAYLOAD_FILE!).text();
const payload = JSON.parse(raw);

/* Only pushes to a branch are interesting. */
const branch = payload.ref?.replace("refs/heads/", "");
if (!branch) process.exit(0);

for (const commit of payload.commits ?? []) {
  console.log(\`  \${commit.id.slice(0, 7)} \${commit.message}\`, 42, true);
}
`,
  json: `{
  "name": "deploy-hook",
  "enabled": true,
  "retries": 3,
  "events": ["adi.tasks.*", null]
}
`,
  py: `# Report the slowest tests of the last run.
import json
from pathlib import Path

report = json.loads(Path("target/test-report.json").read_text())
slow = [t for t in report["tests"] if t["seconds"] > 1.5]

@dataclass
class Row:
    name: str
    seconds: float = 0.0

for test in sorted(slow, key=lambda t: -t["seconds"])[:10]:
    print(f"{test['seconds']:6.2f}s  {test['name']}")
if not slow:
    print('nothing slower than 1.5s')
`,
};
const editorLanguages = [["sh", "Shell"], ["ts", "TypeScript"], ["json", "JSON"], ["py", "Python"]] as const;

const settingsCode = `<SettingsLayout
  title="Settings"
  description="How the workspace behaves."
  actions={<Button variant="outline">Old UI settings</Button>}
  navigation={
    <>
      <SettingsNavItem href="#triggers" active><Zap /> Triggers</SettingsNavItem>
      <SettingsNavItem href="#notifications"><Bell /> Notifications</SettingsNavItem>
    </>
  }
>
  <SettingsSection
    title="Triggers"
    description="Code blocks launched on a webhook call or an event."
    actions={<Button>New trigger</Button>}
  >
    …
  </SettingsSection>
</SettingsLayout>`;

const cardClass = "scroll-mt-36 md:scroll-mt-8";
const exampleSections = [{ id: "triggers", label: "Triggers", icon: Zap }, { id: "notifications", label: "Notifications", icon: Bell }];

/** Gallery entries for the form controls and the settings page layout. */
export function FormExamples() {
  const [enabled, setEnabled] = useState(true);
  const [name, setName] = useState("deploy-hook");
  const [section, setSection] = useState("triggers");
  const [language, setLanguage] = useState<Exclude<CodeLanguage, "plain">>("sh");
  const [code, setCode] = useState(editorSamples);

  return (
    <section aria-labelledby="form-components-title" className="flex min-w-0 flex-col gap-6">
      <div>
        <h2 id="form-components-title" className="text-xl font-semibold tracking-tight">Forms &amp; settings</h2>
        <p className="text-muted-foreground mt-2 text-sm leading-6">Text inputs with their label, help and error; an on/off switch; state badges; and the layout of a settings page.</p>
      </div>

      <Card id="badge-example" tabIndex={-1} className={cardClass}>
        <CardHeader>
          <CardTitle>Badge</CardTitle>
          <CardDescription>A short label for a kind or a state. The word says the state; the dot only repeats it.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <div className="flex flex-wrap items-center gap-2">
            <Badge>Webhook</Badge>
            <Badge variant="outline">sh</Badge>
            <Badge variant="default">New</Badge>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant="success">Running</Badge>
            <Badge variant="warning">Stopped</Badge>
            <Badge variant="destructive">Failed</Badge>
            <Badge variant="muted">Disabled</Badge>
          </div>
          <CodeExample code={badgeCode} label="Badge" />
        </CardContent>
      </Card>

      <Card id="field-example" tabIndex={-1} className={cardClass}>
        <CardHeader>
          <CardTitle>Field, Input &amp; Textarea</CardTitle>
          <CardDescription>A Field labels its control and wires its help and error text to it. The mono face is for machine strings.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <div className="grid gap-5 sm:grid-cols-2">
            <Field label="Name" hint="Letters, digits, '.', '-' or '_'.">
              {(control) => <Input {...control} mono placeholder="deploy-hook" value={name} onChange={(event) => setName(event.target.value)} />}
            </Field>
            <Field label="Interval" error="Enter a whole number of seconds.">
              {(control) => <Input {...control} mono value="soon" readOnly />}
            </Field>
            <Field label="Code block" hint="Run as sh -c." className="sm:col-span-2">
              {(control) => <Textarea {...control} mono rows={4} defaultValue={'curl -fsS "$ADI_URL" | sh'} spellCheck={false} />}
            </Field>
          </div>
          <CodeExample code={fieldCode} label="Field, Input and Textarea" />
        </CardContent>
      </Card>

      <Card id="code-editor-example" tabIndex={-1} className={cardClass}>
        <CardHeader>
          <CardTitle>Code editor</CardTitle>
          <CardDescription>
            A highlighted view of the text with a real textarea on top, so typing, undo, paste and the input method are the browser’s own.
            Tab indents, Shift+Tab outdents, Enter keeps the indentation; Escape then Tab leaves the editor.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <div role="group" aria-label="Example language" className="flex flex-wrap gap-1">
            {editorLanguages.map(([id, label]) => (
              <Button key={id} variant="ghost" size="sm" aria-pressed={language === id} className={cn(language === id && "bg-muted text-foreground")} onClick={() => setLanguage(id)}>{label}</Button>
            ))}
          </div>
          <Field label="Code block" hint="The language can change while the editor stays mounted.">
            {(control) => <CodeEditor {...control} language={language} rows={12} value={code[language]} onValueChange={(next) => setCode((current) => ({ ...current, [language]: next }))} />}
          </Field>
          <CodeExample code={editorCode} label="Code editor" />
        </CardContent>
      </Card>

      <Card id="switch-example" tabIndex={-1} className={cardClass}>
        <CardHeader>
          <CardTitle>Switch</CardTitle>
          <CardDescription>An on/off control that takes effect immediately. The caller owns its state.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <div className="flex items-center gap-3">
            <Switch aria-label="Notifications enabled" checked={enabled} onCheckedChange={setEnabled} />
            <span className="text-sm">Notifications are {enabled ? "on" : "off"}</span>
            <Switch aria-label="Unavailable" checked={false} disabled className="ml-6" />
            <span className="text-muted-foreground text-sm">Disabled</span>
          </div>
          <CodeExample code={switchCode} label="Switch" />
        </CardContent>
      </Card>

      <Card id="settings-layout-example" tabIndex={-1} className={cardClass}>
        <CardHeader>
          <CardTitle>Settings layout</CardTitle>
          <CardDescription>A title row, the list of sections, and the open section. The list becomes a scrolling strip on phones.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <div className="rounded-lg border p-5">
            <SettingsLayout
              title="Settings"
              description="How the workspace behaves."
              actions={<Button variant="outline">Old UI settings <ArrowUpRight className="size-3.5" aria-hidden="true" /></Button>}
              navigationLabel="Example settings sections"
              navigation={(
                <>
                  {exampleSections.map(({ id, label, icon: Icon }) => (
                    <SettingsNavItem key={id} asChild active={section === id}>
                      <button type="button" onClick={() => setSection(id)}><Icon aria-hidden="true" />{label}</button>
                    </SettingsNavItem>
                  ))}
                </>
              )}
            >
              <SettingsSection
                title={section === "triggers" ? "Triggers" : "Notifications"}
                description={section === "triggers" ? "Code blocks launched on a webhook call or an event." : "Where the workspace reports what happened."}
                actions={section === "triggers" ? <Button>New trigger</Button> : undefined}
              >
                <div className="text-muted-foreground rounded-lg border border-dashed p-6 text-sm">The open section’s content.</div>
              </SettingsSection>
            </SettingsLayout>
          </div>
          <CodeExample code={settingsCode} label="Settings layout" />
        </CardContent>
      </Card>
    </section>
  );
}
