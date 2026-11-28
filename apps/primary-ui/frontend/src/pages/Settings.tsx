import { useEffect, type ReactNode } from "react";
import { ArrowUpRight, KeyRound, Zap, type LucideIcon } from "lucide-react";
import { Link, Navigate, NavLink, useParams } from "react-router-dom";

import { Button, SettingsLayout, SettingsNavItem } from "@design-system/ui";

import { SecretsSettings } from "../components/settings/SecretsSettings";
import { TriggersSettings } from "../components/settings/TriggersSettings";
import { appHref } from "../lib/app-api";
import { sourceName } from "../lib/app-chat-sources";
import { useSettingsSource } from "../lib/settings-source";

type Section = { id: string; label: string; icon: LucideIcon; element: ReactNode };

/** The sections this page has so far. Everything else still lives in the old settings. */
const sections: Section[] = [
  { id: "triggers", label: "Triggers", icon: Zap, element: <TriggersSettings /> },
  { id: "secrets", label: "Secrets", icon: KeyRound, element: <SecretsSettings /> },
];

export function Settings() {
  const { section: sectionId } = useParams();
  const [source] = useSettingsSource();
  const section = sections.find((item) => item.id === sectionId);

  useEffect(() => {
    document.title = section ? `${section.label} — Settings — ADI` : "Settings — ADI";
  }, [section]);

  if (!sectionId) return <Navigate to={`/settings/${sections[0].id}`} replace />;

  return (
    <main aria-label="Settings" className="w-full py-3 sm:px-4 sm:py-6 lg:px-6 lg:py-10">
      <SettingsLayout
        title="Settings"
        description={
          <>
            How ADI behaves on{" "}
            <span data-slot="settings-machine">{source ? sourceName(source) : "this machine"}</span>
            . Sections move here one at a time; the rest is still in the old settings.
          </>
        }
        actions={
          <Button asChild variant="outline" data-slot="settings-old-ui">
            <a href={appHref("/extended/settings")}>
              Old UI settings
              <ArrowUpRight className="size-3.5" aria-hidden="true" />
            </a>
          </Button>
        }
        navigation={sections.map(({ id, label, icon: Icon }) => (
          <SettingsNavItem key={id} asChild>
            <NavLink to={`/settings/${id}`}>
              <Icon aria-hidden="true" />
              {label}
            </NavLink>
          </SettingsNavItem>
        ))}
      >
        {section ? (
          section.element
        ) : (
          <div role="alert" className="rounded-xl border border-dashed p-8 text-center">
            <h3 className="font-medium">No such settings section</h3>
            <p className="text-muted-foreground mt-2 text-sm">
              “{sectionId}” is not a section of the new settings. It may still be in the old
              settings.
            </p>
            <div className="mt-5 flex flex-wrap justify-center gap-2">
              <Button asChild variant="outline">
                <Link to={`/settings/${sections[0].id}`}>Open {sections[0].label}</Link>
              </Button>
              <Button asChild variant="ghost">
                <a href={appHref("/extended/settings")}>Old UI settings</a>
              </Button>
            </div>
          </div>
        )}
      </SettingsLayout>
    </main>
  );
}
