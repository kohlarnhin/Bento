/// <reference types="vite/client" />
const counters = import.meta.env.DEV ? { app: 0, projectSidebar: 0 } : undefined;

export function recordRender(component: 'app' | 'projectSidebar') {
  if (counters) counters[component]++;
}

// Like logDeliveryMetrics, this is read by the real-webview harness only.
export function renderMetrics(reset = false) {
  const snapshot = counters && { ...counters };
  if (reset && counters) {
    counters.app = 0;
    counters.projectSidebar = 0;
  }
  return snapshot;
}
