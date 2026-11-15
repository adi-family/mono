import * as React from "react";
import { Slot } from "@radix-ui/react-slot";

import { cn } from "../lib/cn";

type SettingsLayoutProps = Omit<React.ComponentProps<"div">, "title"> & {
  title: React.ReactNode;
  description?: React.ReactNode;
  /** Page-level actions, shown opposite the title on every width. */
  actions?: React.ReactNode;
  /** The section list: `SettingsNavItem`s. */
  navigation: React.ReactNode;
  navigationLabel?: string;
};

/**
 * A settings page: a title row, a list of sections, and the open section. The section list is a
 * column beside the content on wide screens and a scrolling strip above it on phones.
 */
function SettingsLayout({ title, description, actions, navigation, navigationLabel = "Settings sections", className, children, ...props }: SettingsLayoutProps) {
  return (
    <div data-slot="settings" className={cn("mx-auto w-full max-w-6xl", className)} {...props}>
      <header data-slot="settings-header" className="flex flex-wrap items-start justify-between gap-x-6 gap-y-3">
        <div className="min-w-0">
          <h2 className="text-2xl font-semibold tracking-tight sm:text-3xl">{title}</h2>
          {description && <p className="text-muted-foreground mt-2 max-w-xl text-sm leading-relaxed">{description}</p>}
        </div>
        {actions && (
          <div data-slot="settings-actions" className="flex shrink-0 flex-wrap items-center gap-2">
            {actions}
          </div>
        )}
      </header>
      <div className="mt-6 grid gap-6 md:mt-8 md:grid-cols-[12rem_minmax(0,1fr)] md:gap-10">
        <nav
          data-slot="settings-nav"
          aria-label={navigationLabel}
          className="-mx-1 flex gap-1 overflow-x-auto px-1 pb-1 md:sticky md:top-4 md:mx-0 md:flex-col md:self-start md:overflow-visible md:p-0"
        >
          {navigation}
        </nav>
        <div data-slot="settings-content" className="min-w-0">
          {children}
        </div>
      </div>
    </div>
  );
}

type SettingsNavItemProps = React.ComponentProps<"a"> & {
  /** Render the child (a router link) instead of an anchor. */
  asChild?: boolean;
  /** Mark the open section when the child cannot set `aria-current` itself. */
  active?: boolean;
};

/** One section in the settings list. A link: the open one carries `aria-current="page"`. */
function SettingsNavItem({ asChild = false, active, className, ...props }: SettingsNavItemProps) {
  const Comp = asChild ? Slot : "a";
  return (
    <Comp
      data-slot="settings-nav-item"
      {...(active ? { "aria-current": "page" as const } : {})}
      className={cn(
        "text-muted-foreground hover:bg-accent hover:text-accent-foreground focus-visible:ring-ring flex shrink-0 items-center gap-2 rounded-md px-2.5 py-1.5 text-sm font-medium whitespace-nowrap outline-none transition-colors focus-visible:ring-2 aria-[current=page]:bg-accent aria-[current=page]:text-foreground [&_svg]:size-4 [&_svg]:shrink-0",
        className,
      )}
      {...props}
    />
  );
}

type SettingsSectionProps = Omit<React.ComponentProps<"section">, "title"> & {
  title: React.ReactNode;
  description?: React.ReactNode;
  /** Section-level actions, opposite the title. */
  actions?: React.ReactNode;
};

/** One section's heading row and content. */
function SettingsSection({ title, description, actions, className, children, ...props }: SettingsSectionProps) {
  const titleId = React.useId();
  return (
    <section data-slot="settings-section" aria-labelledby={titleId} className={cn("min-w-0", className)} {...props}>
      <header className="mb-5 flex flex-wrap items-start justify-between gap-x-6 gap-y-3">
        <div className="min-w-0">
          <h3 id={titleId} className="text-lg font-semibold tracking-tight">
            {title}
          </h3>
          {description && <p className="text-muted-foreground mt-1 max-w-2xl text-sm leading-relaxed">{description}</p>}
        </div>
        {actions && <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div>}
      </header>
      {children}
    </section>
  );
}

export { SettingsLayout, SettingsNavItem, SettingsSection, type SettingsLayoutProps, type SettingsNavItemProps, type SettingsSectionProps };
