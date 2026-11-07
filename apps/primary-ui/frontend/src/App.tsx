// Agent-authored, never touched by migration. Add a page under `src/pages/` and a `<Route>`
// for it here — see ../README.md.

import { Navigate, Route, Routes, useLocation } from "react-router-dom";
import { lazy, Suspense } from "react";

import { ShowcaseLayout } from "./components/ShowcaseLayout";
import { Home } from "./pages/Home";
import { AppChatProvider } from "./lib/app-chat-state";
import { legacyChatDestination } from "./lib/primary-routes";

const Components = lazy(() => import("./pages/Components").then((module) => ({ default: module.Components })));
const Interface = lazy(() => import("./pages/Interface").then((module) => ({ default: module.Interface })));
const AgentChat = lazy(() => import("./pages/AgentChat").then((module) => ({ default: module.AgentChat })));

const Marketplace = lazy(() => import("./pages/Marketplace").then((module) => ({ default: module.Marketplace })));

const MarketplacePackage = lazy(() => import("./pages/MarketplacePackage").then((module) => ({ default: module.MarketplacePackage })));

function HomeEntry() {
  const { key } = useLocation();
  // A fresh mobile visit starts with the inbox; Home remains reachable from the tabs.
  if (key === "default" && window.matchMedia("(max-width: 767px)").matches) {
    return <Navigate to="/interface" replace />;
  }
  return <Home />;
}

function LegacyChatRedirect() {
  const { pathname } = useLocation();
  return <Navigate to={legacyChatDestination(pathname) ?? "/"} replace />;
}

export default function App() {
  const { pathname, search, hash } = useLocation();
  if (pathname.length > 1 && pathname.endsWith("/")) {
    return <Navigate to={`${pathname.replace(/\/+$/, "")}${search}${hash}`} replace />;
  }

  return (
    <AppChatProvider>
      <Suspense fallback={<p role="status" className="text-muted-foreground p-5 text-sm">Loading…</p>}>
        <Routes>
          <Route path="/index.html" element={<Navigate to="/" replace />} />
          <Route path="/chat/*" element={<LegacyChatRedirect />} />
          <Route element={<ShowcaseLayout />}>
            <Route path="/" element={<HomeEntry />} />
            <Route path="/marketplace" element={<Marketplace />} />
            <Route path="/marketplace/:marketplace/:slug" element={<MarketplacePackage />} />
            <Route path="/library" element={<Components />} />
            <Route path="/library/agents/layout-reviewer" element={<AgentChat />} />
            <Route path="/interface" element={<Interface />} />
          </Route>
        </Routes>
      </Suspense>
    </AppChatProvider>
  );
}
