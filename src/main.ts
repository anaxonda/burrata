import { mount } from "svelte";
import App from "./App.svelte";
import { call, desktop } from "./lib/api";
import { installNativeDropdowns } from "./lib/native-dropdown";
import "./style.css";
import "./desktop-layout.css";
import "./player.css";
import "./dropdown.css";
const target = document.getElementById("app")!;

function resetBurrataStorage(storage: Storage) {
  for (const key of Object.keys(storage)) {
    if (/^burrata[.:-]/.test(key)) storage.removeItem(key);
  }
}

async function start() {
  let startupWarning = "";
  if (desktop()) {
    try {
      const status = await call<{ resetId: string | null; warning: string | null }>("get_factory_reset_status");
      startupWarning = status.warning ?? "";
      if (status.resetId) {
        resetBurrataStorage(localStorage);
        resetBurrataStorage(sessionStorage);
        await call("ack_factory_reset", { resetId: status.resetId });
      }
    } catch {
      const message = document.createElement("p");
      message.setAttribute("role", "alert");
      message.textContent = "Could not verify Burrata’s startup state. Restart Burrata to retry.";
      target.replaceChildren(message);
      return;
    }
  }
  installNativeDropdowns(document.body);
  mount(App, { target, props: { startupWarning } });
}

void start();
