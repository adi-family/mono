// Agent-authored, never touched by migration. Add a page under `src/pages/` and a `<Route>`
// for it here — see ../README.md.

import { Route, Routes } from "react-router-dom";

import { Home } from "./pages/Home";

export default function App() {
  return (
    <Routes>
      <Route path="/" element={<Home />} />
    </Routes>
  );
}
