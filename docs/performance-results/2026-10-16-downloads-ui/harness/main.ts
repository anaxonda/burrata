import { mount } from "svelte";
import "/home/loki/projects/burrata/src/style.css";
import "/home/loki/projects/burrata/src/desktop-layout.css";
import "/home/loki/projects/burrata/src/player.css";
import Fixture from "./Fixture.svelte";
mount(Fixture, { target: document.getElementById("app")! });
