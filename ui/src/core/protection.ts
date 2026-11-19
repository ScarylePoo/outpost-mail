// The protection settings, shared by everything that shows a message, and
// the one place the settings password is asked for.
//
// With a password set, the core refuses to change a setting or to override
// a warning without it. `withPassword` runs such a change: it asks for the
// password when one is set, and asks again if it was wrong.

import { createSignal } from "solid-js";
import { core } from "./index";
import type { General, Protection } from "./api";

const DEFAULTS: Protection = {
  disableFailed: true,
  blockRemoteImages: true,
  allowImagesFrom: [],
  firstTimeBanner: true,
  lookalikeWarning: true,
  replyWarning: true,
  showLinkDestination: true,
  recheckSignatures: true,
  blockedSenders: [],
};

const [general, setGeneral] = createSignal<General>({
  notifyNewMail: true,
  startMinimised: false,
  closeToTray: false,
  undoSendSeconds: 10,
  conversations: true,
  previewLines: 3,
  checkForUpdates: true,
});
const [protection, setProtection] = createSignal<Protection>(DEFAULTS);
const [passwordSet, setPasswordSet] = createSignal(false);
/** Counts changes that affect how an open message is shown. */
const [protectionVersion, setProtectionVersion] = createSignal(0);

/** Counts subjects the user has changed, so that what is open is read again. */
const [subjectsVersion, setSubjectsVersion] = createSignal(0);
export const subjectsChanged = () => setSubjectsVersion((v) => v + 1);

export { general, protection, passwordSet, protectionVersion, subjectsVersion };

export function loadProtection(): Promise<void> {
  void core
    .send({ type: "getGeneral" })
    .then((r) => setGeneral(r.general))
    .catch(() => {});
  return core
    .send({ type: "getProtection" })
    .then((r) => {
      setProtection(r.protection);
      setPasswordSet(r.passwordSet);
    })
    .catch(() => {});
}

/** What the password prompt is showing, if it is open. */
export interface Asking {
  /** What the password is needed for, as a sentence. */
  why: string;
  /** Set after a wrong attempt. */
  error: string;
  answer: (password: string | null) => void;
}

const [asking, setAsking] = createSignal<Asking>();
export { asking };

function ask(why: string, error: string): Promise<string | null> {
  return new Promise((resolve) => {
    setAsking({
      why,
      error,
      answer: (password) => {
        setAsking(undefined);
        resolve(password);
      },
    });
  });
}

/**
 * Runs a change that the settings password guards. Resolves to `undefined`
 * if the user gave up at the password prompt.
 */
export async function withPassword<T>(why: string, change: (password: string | null) => Promise<T>): Promise<T | undefined> {
  if (!passwordSet()) return change(null);
  let error = "";
  for (;;) {
    const password = await ask(why, error);
    if (password === null) return undefined;
    try {
      return await change(password);
    } catch (e) {
      if ((e as { code?: string })?.code !== "wrongPassword") throw e;
      error = "That is not the password.";
    }
  }
}

/** Changes the protection settings. Resolves to whether they were changed. */
export async function saveProtection(why: string, next: Protection): Promise<boolean> {
  const done = await withPassword(why, (password) => core.send({ type: "setProtection", protection: next, password }));
  if (!done) return false;
  setProtection(done.protection);
  setPasswordSet(done.passwordSet);
  setProtectionVersion((v) => v + 1);
  return true;
}

/** Changes the general settings. Resolves to whether they were changed. */
export async function saveGeneral(next: General): Promise<boolean> {
  const done = await withPassword("To change the settings", (password) => core.send({ type: "setGeneral", general: next, password }));
  if (!done) return false;
  setGeneral(done.general);
  return true;
}

/** Sets, changes or (with an empty `next`) removes the settings password. */
export async function savePassword(current: string | null, next: string): Promise<void> {
  const done = await core.send({ type: "setProtectionPassword", current, new: next || null });
  setPasswordSet(done.passwordSet);
}

/**
 * What the reading pane can ask the main window to do with the message on
 * show. Set by the main window.
 */
export const messageActions: {
  remove?: () => void;
  moveToJunk?: () => void;
  /** Whether the account has a junk folder to move to. */
  hasJunk?: () => boolean;
} = {};
