import { Link, useLocation } from "react-router-dom";

import { ScrollSidebar, type ScrollSidebarGroup } from "@design-system/ui";

const groups: readonly ScrollSidebarGroup[] = [
  {
    label: "Chat",
    links: [
      { id: "chat-thread-example", label: "Conversation" },
      { id: "chat-tools-example", label: "Tool calls" },
      { id: "chat-messages-example", label: "Messages & typing" },
      { id: "chat-composer-example", label: "Composer" },
    ],
  },
  {
    label: "Selection & search",
    links: [
      { id: "select-example", label: "Select" },
      { id: "multi-select-example", label: "Multi select" },
      { id: "autocomplete-example", label: "Autocomplete" },
      { id: "async-autocomplete-example", label: "Async autocomplete" },
      { id: "creatable-autocomplete-example", label: "Create an option" },
      { id: "async-creatable-example", label: "Async + create" },
      { id: "async-multi-select-example", label: "Async multi select" },
    ],
  },
  {
    label: "Other components",
    links: [
      { id: "app-layout-example", label: "App layout" },
      { id: "scroll-sidebar-example", label: "Scroll sidebar" },
      { id: "button-example", label: "Button" },
      { id: "help-components", label: "Question mark & help" },
      { id: "disabled-buttons", label: "Disabled buttons" },
      { id: "theme-toggle-example", label: "Theme toggle" },
      { id: "card-example", label: "Card" },
    ],
  },
];

export function ComponentSidebar({ className }: { className?: string }) {
  const { pathname, key } = useLocation();

  return (
    <ScrollSidebar
      className={className}
      groups={groups}
      title="Components"
      browseLabel="Browse components"
      navigationLabel="Component navigation"
      enabled={pathname === "/library"}
      hrefPrefix="/library"
      navigationKey={key}
      renderLink={({ href, ...props }) => <Link to={href} {...props} />}
    />
  );
}
