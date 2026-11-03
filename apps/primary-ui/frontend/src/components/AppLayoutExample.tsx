import { useState } from "react";
import { Blocks } from "lucide-react";

import {
  AppLayout,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Select,
} from "@design-system/ui";

import { AdiLogo } from "./AdiLogo";
import { CodeExample } from "./CodeExample";

const layoutCode = `<AppLayout
  surface="joined"
  surfaceProps={{ className: "dark:bg-black" }}
  primaryNavigation={<GlobalNavigation />}
  secondaryNavigation={<SectionNavigation />}
  secondaryNavigationProps={{ className: "bg-muted/50 dark:bg-background" }}
  topbar={<h1>Workspace</h1>}
>
  <main>Page content</main>
</AppLayout>`;

const iconLinkClass = "focus-visible:ring-ring flex size-10 items-center justify-center rounded-lg outline-none transition-colors hover:bg-foreground/5 focus-visible:ring-2";

export function AppLayoutExample() {
  const [surface, setSurface] = useState<"separate" | "joined">("joined");

  return (
    <Card id="app-layout-example" tabIndex={-1} className="scroll-mt-36 md:scroll-mt-8">
      <CardHeader>
        <CardTitle>App layout</CardTitle>
        <CardDescription>
          Two navigation levels with a separate content panel or one shared workspace surface.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="flex items-center gap-3">
          <label htmlFor="app-layout-surface-mode" className="text-sm font-medium">Surface</label>
          <div className="w-44">
            <Select
              id="app-layout-surface-mode"
              value={surface}
              onValueChange={(value) => setSurface(value as "separate" | "joined")}
              options={[
                { value: "joined", label: "Joined workspace" },
                { value: "separate", label: "Separate panel" },
              ]}
            />
          </div>
        </div>
        <div className="bg-canvas isolate overflow-hidden rounded-lg border [overflow-wrap:anywhere]" aria-label="App layout preview">
          <AppLayout
            className="[--app-layout-height:24rem]"
            surface={surface}
            surfaceProps={surface === "joined" ? { className: "dark:bg-black" } : undefined}
            secondaryNavigationProps={surface === "joined" ? { className: "bg-muted/50 dark:bg-background" } : undefined}
            primaryNavigation={
              <nav aria-label="Preview global navigation" className="flex w-14 flex-col items-center">
                <div className="flex h-14 items-center justify-center">
                  <a href="/" aria-label="ADI home" title="ADI home" className={iconLinkClass}>
                    <AdiLogo />
                  </a>
                </div>
                <a href="#app-layout-example" aria-label="App layout example" title="App layout example" className={`${iconLinkClass} bg-foreground/10`}>
                  <Blocks aria-hidden="true" className="size-5" />
                </a>
              </nav>
            }
            secondaryNavigation={
              <nav aria-label="Preview section navigation" className="min-w-0">
                <p className="mb-3 truncate px-2 py-1 text-sm font-semibold">Components</p>
                <div className="grid grid-cols-1 gap-1">
                  <a href="#app-layout-example" className="bg-accent text-accent-foreground focus-visible:ring-ring rounded-md px-2 py-1.5 text-sm font-medium outline-none focus-visible:ring-2">
                    App layout
                  </a>
                  <a href="#button-example" className="text-muted-foreground hover:bg-accent/60 hover:text-foreground focus-visible:ring-ring rounded-md px-2 py-1.5 text-sm outline-none focus-visible:ring-2">
                    Button
                  </a>
                </div>
              </nav>
            }
            topbar={<span className="text-sm font-medium">Top bar</span>}
          >
            <section aria-label="Preview content" className="flex h-full min-h-32 flex-col justify-center gap-2">
              <h3 className="text-sm font-medium">Content</h3>
              <p className="text-muted-foreground text-sm">Your page goes here.</p>
            </section>
          </AppLayout>
        </div>
        <p className="text-muted-foreground text-sm">
          The joined surface has a slightly wider, shaded sidebar beside the content, with one
          outer corner. The top bar stays outside, above the surface. Drag the sidebar’s right edge to resize it;
          double-click to reset. You can also focus the edge and use the arrow keys.
        </p>
        <CodeExample code={layoutCode} label="App layout" />
      </CardContent>
    </Card>
  );
}
