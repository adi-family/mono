// adi-vite-frontend-entry: 1 — this file's generation. The control panel replaces any copy that
// does not spell the current one (`handlers/dashboards.rs`); bump it whenever this file changes.
// It is generated, so a hand-edited one is replaced — the dashboard's own UI belongs in
// `src/App.tsx` and `src/pages/`, which migration never reads.

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter } from "react-router-dom";

import App from "./App";
import "./index.css";

const queryClient = new QueryClient();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <App />
      </BrowserRouter>
    </QueryClientProvider>
  </StrictMode>,
);
