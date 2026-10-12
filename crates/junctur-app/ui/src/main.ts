import { mount } from "svelte";
import App from "./App.svelte";
import { uiLog } from "./lib/api";
import "./app.css";

// Script errors go to the app's log too: an off-screen test run has no console to look at.
window.addEventListener("error", (e) => void uiLog(`error: ${e.message}`).catch(() => {}));
window.addEventListener("unhandledrejection", (e) => void uiLog(`unhandled: ${e.reason}`).catch(() => {}));

mount(App, { target: document.getElementById("app")! });
