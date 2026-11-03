import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  ScrollSidebar,
  type ScrollSidebarGroup,
} from "@design-system/ui";

import { CodeExample } from "./CodeExample";

const groups: readonly ScrollSidebarGroup[] = [
  {
    label: "Examples",
    links: [
      { id: "scroll-sidebar-example", label: "Scroll sidebar" },
      { id: "button-example", label: "Button" },
      { id: "card-example", label: "Card" },
    ],
  },
];

const sidebarCode = `const groups = [{
  label: "Guide",
  links: [
    { id: "overview", label: "Overview" },
    { id: "details", label: "Details" },
  ],
}];

<ScrollSidebar title="Guide" groups={groups} />

<section id="overview" tabIndex={-1} className="scroll-mt-8">
  Overview
</section>
<section id="details" tabIndex={-1} className="scroll-mt-8">
  Details
</section>`;

export function ScrollSidebarExample() {
  return (
    <Card id="scroll-sidebar-example" tabIndex={-1} className="scroll-mt-36 md:scroll-mt-8">
      <CardHeader>
        <CardTitle>Scroll sidebar</CardTitle>
        <CardDescription>
          Grouped section links with automatic selection as you scroll. The gallery navigation uses this component.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="bg-canvas w-full max-w-xs rounded-lg border p-3">
          <ScrollSidebar title="Try the links" browseLabel="Browse examples" navigationLabel="Sidebar example navigation" groups={groups} />
        </div>
        <p className="text-muted-foreground text-sm">
          On small screens, the links collapse into a menu. Select a section to jump to it;
          its selection stays visible even when a short section is near the end of the page.
        </p>
        <CodeExample code={sidebarCode} label="Scroll sidebar" />
      </CardContent>
    </Card>
  );
}
