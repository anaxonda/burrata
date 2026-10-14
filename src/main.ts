import { mount } from "svelte";
import App from "./App.svelte";
import "./style.css";
import "./desktop-layout.css";
import "./player.css";
mount(App, { target: document.getElementById("app")! });
