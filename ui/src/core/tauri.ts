// Tauri adapter, interface side. The Rust half is crates/shell-tauri/src/main.rs.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Command, CoreEvent, ResponseTo, Transport } from "./api";

export const tauriTransport: Transport = {
  name: "tauri",
  send<C extends Command>(command: C) {
    return invoke<ResponseTo<C>>("core_command", { command });
  },
  openLink(url) {
    return invoke("open_link", { url });
  },
  revealDownload(path) {
    return invoke("reveal_download", { path });
  },
  notify(title, body) {
    return invoke("notify", { title, body });
  },
  setUnread(count) {
    return invoke("set_unread", { count });
  },
  onMailto(listener) {
    let stop: (() => void) | undefined;
    let stopped = false;
    void invoke<string | null>("launch_mailto").then((link) => {
      if (link && !stopped) listener(link);
    });
    void listen<string>("mailto", (e) => listener(e.payload)).then((unlisten) => {
      if (stopped) unlisten();
      else stop = unlisten;
    });
    return () => {
      stopped = true;
      stop?.();
    };
  },
  chooseDefaultMailApp() {
    return invoke("choose_default_mail_app");
  },
  revealLog() {
    return invoke("reveal_log");
  },
  setZoom(factor) {
    return invoke("set_zoom", { factor });
  },
  onEvent(listener) {
    let stop: (() => void) | undefined;
    let stopped = false;
    void listen<CoreEvent>("core-event", (e) => listener(e.payload)).then((unlisten) => {
      if (stopped) unlisten();
      else stop = unlisten;
    });
    return () => {
      stopped = true;
      stop?.();
    };
  },
};
