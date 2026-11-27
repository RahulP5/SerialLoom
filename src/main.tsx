import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@xyflow/react/dist/style.css";
import "./index.css";
import App from "./App";
import { TraceWindow } from "./components/views/TraceWindow";
import type { TraceOptions } from "./lib/trace";
import { initTheme } from "./lib/theme";
import { initI18n } from "./i18n";

const trace = (window as Window & { __SERIALLOOM_TRACE__?: TraceOptions }).__SERIALLOOM_TRACE__;

initTheme();

void initI18n().then(() =>
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      {trace ? <TraceWindow options={trace} /> : <App />}
    </StrictMode>,
  ),
);
