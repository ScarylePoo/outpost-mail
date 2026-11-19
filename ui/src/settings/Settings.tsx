// The settings window. A list of sections on the left, the chosen section on
// the right in groups, and a search box that looks through every section at
// once and shows whatever matches. One level deep: no section opens another.
// It takes the place of the list and reading pane while open; nothing here
// is modal.

import { createMemo, createSignal, For, type JSX, onCleanup, onMount, Show } from "solid-js";
import { core, type AccountInfo, type Appearance, type DavAccountInfo, type General, type Protection } from "../core";
import {
  ACCENTS,
  appearance,
  DEFAULT_APPEARANCE,
  fontInstalled,
  installedFonts,
  READING_SIZES,
  saveAppearance,
  SCHEMES,
  TEXT_SIZES,
} from "../core/appearance";
import { checkForUpdate, update, updateChecked } from "../core/updates";
import { davAccounts } from "../organizer/store";
import { general, passwordSet, protection, saveGeneral, savePassword, saveProtection, withPassword } from "../core/protection";
import { SHORTCUTS } from "../shortcuts";

function errorText(e: unknown): string {
  return String((e as { message?: string })?.message ?? e);
}

/** One setting: the words it is found by, and how it is drawn. */
interface Item {
  words: string;
  view: () => JSX.Element;
}

/** Settings that belong together, drawn as one card. */
interface Group {
  title?: string;
  /** Rows close together: a table of short lines. */
  tight?: boolean;
  items: Item[];
}

/** One entry in the list on the left. */
interface Page {
  id: string;
  title: string;
  /** The heading it is listed under: accounts are. */
  under?: string;
  groups: Group[];
}

type GeneralSwitch = "notifyNewMail" | "startMinimised" | "closeToTray" | "conversations" | "checkForUpdates";
type ProtectionSwitch = Exclude<keyof Protection, "allowImagesFrom" | "blockedSenders">;

const PROTECTION: { key: ProtectionSwitch; label: string; hint: string }[] = [
  {
    key: "disableFailed",
    label: "Turn off links and attachments on messages that fail a sender check",
    hint: "Each link is replaced by its text and where it pointed. Attachments cannot be saved.",
  },
  {
    key: "blockRemoteImages",
    label: "Block pictures from the internet",
    hint: "Loading a picture tells its sender that you opened the message, and when. Senders you allow are listed below.",
  },
  { key: "firstTimeBanner", label: "Say when a message is the first from its sender", hint: "" },
  {
    key: "lookalikeWarning",
    label: "Warn when a sender's domain resembles one you get mail from",
    hint: "Such as a bank's name with one letter changed.",
  },
  { key: "replyWarning", label: "Warn when replies would go to a different domain than the sender's", hint: "" },
  {
    key: "showLinkDestination",
    label: "Always show where a link really goes before opening it",
    hint: "When off, clicking a link opens it straight away.",
  },
  {
    key: "recheckSignatures",
    label: "Check signatures again on this computer",
    hint: "Looks up the signer's key through this computer's DNS, as following a link to their site would. A valid signature from the sender's own domain counts as verified.",
  },
];

function Switch(props: { on: boolean; label: string; hint?: string; change: (on: boolean) => void }) {
  return (
    <>
      <label class="check">
        <input
          type="checkbox"
          checked={props.on}
          onChange={(e) => {
            const wanted = e.currentTarget.checked;
            // Shown as it is until the change has really been made (a
            // password may be asked for, and refused).
            e.currentTarget.checked = props.on;
            props.change(wanted);
          }}
        />
        {props.label}
      </label>
      <Show when={props.hint}>
        <p class="panel-hint">{props.hint}</p>
      </Show>
    </>
  );
}

/** Adding a calendar-and-contacts account. */
function AddDav() {
  const [url, setUrl] = createSignal("");
  const [username, setUsername] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [remember, setRemember] = createSignal(true);
  const [checking, setChecking] = createSignal(false);
  const [note, setNote] = createSignal("");

  onMount(() => {
    const stop = core.onEvent((event) => {
      if (!checking()) return;
      if (event.type === "davAccountAdded") {
        setChecking(false);
        setNote("Added. Its calendars and address books are listed above.");
        setUrl("");
        setUsername("");
        setPassword("");
      } else if (event.type === "davAccountSetupFailed") {
        setChecking(false);
        setNote(`Could not add the account: ${event.error}`);
      }
    });
    onCleanup(stop);
  });

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        setNote("");
        setChecking(true);
        core
          .send({ type: "addDavAccount", url: url(), username: username(), password: password(), remember: remember() })
          .catch((e) => {
            setChecking(false);
            setNote(errorText(e));
          });
      }}
    >
      <div class="setting-title">Add a calendar and contacts account</div>
      <p class="panel-hint flush">
        For a CalDAV or CardDAV server: Nextcloud, Radicale, Baikal, Fastmail and others. Give the server's address;
        Outpost finds your calendars, task lists and address books on it.
      </p>
      <label>
        Server address
        <input type="text" spellcheck={false} required placeholder="dav.example.com" value={url()} onInput={(e) => setUrl(e.currentTarget.value)} />
      </label>
      <Show when={/^http:\/\//i.test(url().trim())}>
        <p class="panel-warning">
          An address starting with http:// is not encrypted: your password and data cross the network readable by
          anyone in between. Only use it for a server on your own machine.
        </p>
      </Show>
      <div class="panel-row">
        <label class="grow">
          User name
          <input type="text" spellcheck={false} required autocomplete="off" value={username()} onInput={(e) => setUsername(e.currentTarget.value)} />
        </label>
        <label class="grow">
          Password
          <input type="password" required autocomplete="new-password" value={password()} onInput={(e) => setPassword(e.currentTarget.value)} />
        </label>
      </div>
      <label class="check">
        <input type="checkbox" checked={remember()} onChange={(e) => setRemember(e.currentTarget.checked)} />
        Remember the password on this computer
      </label>
      <Show when={note()}>
        <p class="panel-note" role="status">
          {note()}
        </p>
      </Show>
      <button type="submit" disabled={checking()}>
        {checking() ? "Checking…" : "Sign in and add"}
      </button>
    </form>
  );
}

/** Everything about one calendar-and-contacts account. */
function davItems(account: DavAccountInfo, say: (text: string) => void): Item[] {
  const who = `${account.username} ${account.url} calendar contacts account caldav carddav`;
  const kind = (c: DavAccountInfo["collections"][number]) =>
    c.contacts ? "address book" : c.events && c.tasks ? "calendar and tasks" : c.tasks ? "task list" : "calendar";
  const [password, setPassword] = createSignal("");
  const needsPassword = () => account.status.state === "needsPassword" || account.status.state === "signInFailed";
  return [
    {
      words: `${who} status sync`,
      view: () => (
        <div class="setting-line">
          <span>
            {account.url}
            <span class="setting-aside">
              {account.status.state === "upToDate"
                ? "Up to date."
                : account.status.state === "syncing"
                  ? "Syncing…"
                  : account.status.state === "offline"
                    ? `Not reachable: ${account.status.detail}`
                    : account.status.state === "signInFailed"
                      ? "The server refused the password."
                      : account.status.state === "needsPassword"
                        ? "Needs its password."
                        : ""}
            </span>
          </span>
          <button onClick={() => void core.send({ type: "syncDav", account: account.id }).catch((e) => say(errorText(e)))}>Sync now</button>
        </div>
      ),
    },
    ...(needsPassword()
      ? [
          {
            words: `${who} password sign in`,
            view: () => (
              <form
                class="setting-line"
                onSubmit={(e) => {
                  e.preventDefault();
                  core
                    .send({ type: "provideDavPassword", account: account.id, password: password(), remember: true })
                    .then(() => setPassword(""))
                    .catch((e) => say(errorText(e)));
                }}
              >
                <label class="grow">
                  Password for {account.username}
                  <input type="password" required value={password()} onInput={(e) => setPassword(e.currentTarget.value)} />
                </label>
                <button type="submit">Sign in</button>
              </form>
            ),
          } satisfies Item,
        ]
      : []),
    ...account.collections.map(
      (c): Item => ({
        words: `${who} ${c.name} ${kind(c)} colour color show sync`,
        view: () => (
          <div class="setting-line">
            <label class="check">
              <input
                type="checkbox"
                checked={c.enabled}
                onChange={(e) =>
                  void core
                    .send({ type: "setDavCollection", account: account.id, collection: c.id, enabled: e.currentTarget.checked, color: c.color })
                    .catch((err) => say(errorText(err)))
                }
              />
              {c.name}
              <span class="setting-aside inline-aside">{kind(c)}</span>
            </label>
            <Show when={!c.contacts}>
              <input
                type="color"
                aria-label={`Colour of ${c.name}`}
                value={c.color ?? "#5b626c"}
                onChange={(e) =>
                  void core
                    .send({ type: "setDavCollection", account: account.id, collection: c.id, enabled: c.enabled, color: e.currentTarget.value })
                    .catch((err) => say(errorText(err)))
                }
              />
            </Show>
          </div>
        ),
      }),
    ),
    {
      words: `${who} remove delete`,
      view: () => (
        <div class="setting-line">
          <span>
            Remove this account from this computer
            <span class="setting-aside">Its calendars and contacts are deleted here. Nothing changes on the server.</span>
          </span>
          <button
            class="danger"
            onClick={() => {
              if (!window.confirm(`Remove ${account.username} on ${account.url} from this computer? Everything stays on the server.`)) return;
              withPassword(`To remove ${account.username}`, (password) => core.send({ type: "removeDavAccount", account: account.id, password })).catch((e) =>
                say(errorText(e)),
              );
            }}
          >
            Remove account
          </button>
        </div>
      ),
    },
  ];
}

/** Everything about one account. */
function accountItems(
  account: AccountInfo,
  changed: () => void,
  say: (text: string) => void,
  askPassword: (account: string) => void,
): Item[] {
  const id = account.id;
  const who = `${account.name} ${account.username} account`;
  const [host, setHost] = createSignal(account.host);
  const [port, setPort] = createSignal(String(account.port));
  const [encrypted, setEncrypted] = createSignal(account.encrypted);
  const [signature, setSignature] = createSignal(account.signature);
  const [server, setServer] = createSignal(account.checkingServer);
  const s = account.sending;
  const [name, setName] = createSignal(s?.name ?? "");
  const [address, setAddress] = createSignal(s?.address ?? (account.username.includes("@") ? account.username : ""));
  const [smtpHost, setSmtpHost] = createSignal(s?.host ?? account.host);
  const [smtpPort, setSmtpPort] = createSignal(String(s?.port ?? 465));
  const [security, setSecurity] = createSignal<"tls" | "starttls" | "plaintext">(s?.security ?? "tls");

  const update = (what: string) =>
    withPassword(`To change ${account.username}`, (password) =>
      core.send({
        type: "updateAccount",
        account: id,
        host: host(),
        port: Number(port()),
        encrypted: encrypted(),
        signature: signature(),
        password,
      }),
    )
      .then((done) => {
        if (done) {
          say(what);
          changed();
        }
      })
      .catch((e) => say(errorText(e)));

  return [
    {
      words: `${who} incoming server host port encrypted tls imap`,
      view: () => (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void update("The server details are saved.");
          }}
        >
          <div class="panel-row">
            <label class="grow">
              Incoming mail server
              <input type="text" spellcheck={false} required value={host()} onInput={(e) => setHost(e.currentTarget.value)} />
            </label>
            <label class="narrow">
              Port
              <input type="text" inputmode="numeric" required value={port()} onInput={(e) => setPort(e.currentTarget.value)} />
            </label>
          </div>
          <label class="check">
            <input type="checkbox" checked={encrypted()} onChange={(e) => setEncrypted(e.currentTarget.checked)} />
            Encrypted connection (TLS)
          </label>
          <p class="panel-hint">Signs in as {account.username}. To use a different mailbox, add it as another account.</p>
          <button type="submit">Save server</button>
        </form>
      ),
    },
    {
      words: `${who} password sign in`,
      view: () => (
        <div class="setting-line">
          <span>Password for {account.username}</span>
          <button onClick={() => askPassword(id)}>Enter a new password</button>
        </div>
      ),
    },
    {
      words: `${who} sending outgoing server smtp name address identity from`,
      view: () => (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            core
              .send({
                type: "setSending",
                account: id,
                sending: { name: name(), address: address(), host: smtpHost(), port: Number(smtpPort()), security: security() },
              })
              .then(() => {
                say("Sending is set up.");
                changed();
              })
              .catch((e) => say(errorText(e)));
          }}
        >
          <div class="panel-row">
            <label class="grow">
              Your name, as recipients see it
              <input type="text" value={name()} onInput={(e) => setName(e.currentTarget.value)} />
            </label>
            <label class="grow">
              Address mail is sent from
              <input type="text" spellcheck={false} required value={address()} onInput={(e) => setAddress(e.currentTarget.value)} />
            </label>
          </div>
          <div class="panel-row">
            <label class="grow">
              Outgoing mail server
              <input type="text" spellcheck={false} required value={smtpHost()} onInput={(e) => setSmtpHost(e.currentTarget.value)} />
            </label>
            <label class="narrow">
              Port
              <input type="text" inputmode="numeric" required value={smtpPort()} onInput={(e) => setSmtpPort(e.currentTarget.value)} />
            </label>
          </div>
          <label>
            Security
            <select
              value={security()}
              onChange={(e) => {
                const value = e.currentTarget.value as "tls" | "starttls" | "plaintext";
                setSecurity(value);
                setSmtpPort(value === "tls" ? "465" : value === "starttls" ? "587" : "25");
              }}
            >
              <option value="tls">Encrypted from the start (TLS, usually port 465)</option>
              <option value="starttls">Upgraded to encrypted (STARTTLS, usually port 587)</option>
              <option value="plaintext">Not encrypted (only for a server on this computer)</option>
            </select>
          </label>
          <button type="submit">{account.sending ? "Save sending" : "Set up sending"}</button>
        </form>
      ),
    },
    {
      words: `${who} signature`,
      view: () => (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void update("The signature is saved.");
          }}
        >
          <label>
            Signature
            <textarea rows={4} value={signature()} onInput={(e) => setSignature(e.currentTarget.value)} />
          </label>
          <p class="panel-hint flush">Put at the end of each message you start from this account. You can change it in the message.</p>
          <button type="submit">Save signature</button>
        </form>
      ),
    },
    {
      words: `${who} sender checks believed server verification authentication results`,
      view: () => (
        <form
          class="panel-server"
          onSubmit={(e) => {
            e.preventDefault();
            withPassword("To change whose sender checks are believed", (password) =>
              core.send({ type: "setCheckingServer", account: id, server: server(), password }),
            )
              .then((done) => {
                if (!done) return;
                say("Saved.");
                changed();
              })
              .catch((e) => say(errorText(e)));
          }}
        >
          <label>
            Server whose sender checks are believed
            <input
              type="text"
              spellcheck={false}
              placeholder="for example mx.yourhost.com"
              value={server()}
              onInput={(e) => setServer(e.currentTarget.value)}
            />
          </label>
          <button type="submit">{account.checkingServerConfirmed && server() === account.checkingServer ? "Confirmed" : "Confirm"}</button>
          <p class="panel-hint flush">
            Your mail server writes the result of its checks into each message. Anyone can write the same into a message
            they send, so only the server named here is believed, and only what it wrote at the very top.{" "}
            <Show when={!account.checkingServerConfirmed}>
              <Show when={account.checkingServer} fallback="None found yet: this account's recent mail carries no check results.">
                This name was found on most of this account's recent mail; confirm it if it is your mail provider's.
              </Show>
            </Show>
          </p>
        </form>
      ),
    },
    {
      words: `${who} remove delete account`,
      view: () => (
        <div class="setting-line">
          <span>
            Remove this account from this computer
            <span class="setting-aside">Its downloaded mail and stored password are deleted here. Nothing changes on the server.</span>
          </span>
          <button
            class="danger"
            onClick={() => {
              if (!window.confirm(`Remove ${account.username} and delete its downloaded mail from this computer? The mail stays on the server.`)) return;
              withPassword(`To remove ${account.username}`, (password) => core.send({ type: "removeAccount", account: id, password }))
                .then((done) => done && changed())
                .catch((e) => say(errorText(e)));
            }}
          >
            Remove account
          </button>
        </div>
      ),
    },
  ];
}

export function Settings(props: {
  accounts: AccountInfo[];
  coreVersion: string;
  /** What the search box starts with: a way to open the page at one topic. */
  find?: string;
  /** The section to open at, by id. */
  page?: string;
  onAccountsChanged: () => void;
  onAddAccount: () => void;
  onPassword: (account: string) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = createSignal(props.find ?? "");
  const [note, setNote] = createSignal("");
  const [current, setCurrent] = createSignal("");
  const [next, setNext] = createSignal("");
  const [repeat, setRepeat] = createSignal("");
  const [passwordNote, setPasswordNote] = createSignal("");

  const changeGeneral = (next: General) => void saveGeneral(next).catch((e) => setNote(errorText(e)));
  const changeProtection = (next: Protection) =>
    void saveProtection("To change the protection settings", next).catch((e) => setNote(errorText(e)));
  const generalSwitch = (key: GeneralSwitch, label: string, hint: string): Item => ({
    words: `${label} ${hint}`,
    view: () => <Switch on={general()[key]} label={label} hint={hint} change={(on) => changeGeneral({ ...general(), [key]: on })} />,
  });

  async function setPassword(e: Event) {
    e.preventDefault();
    setPasswordNote("");
    if (next() !== repeat()) {
      setPasswordNote("The two new passwords are not the same.");
      return;
    }
    try {
      await savePassword(passwordSet() ? current() : null, next());
      setPasswordNote(next() ? "The password is set." : "The password has been removed.");
      setCurrent("");
      setNext("");
      setRepeat("");
    } catch (e) {
      setPasswordNote((e as { code?: string })?.code === "wrongPassword" ? "The current password is not right." : errorText(e));
    }
  }

  const list = (key: "allowImagesFrom" | "blockedSenders", title: string, none: string, remove: string): Item => ({
    words: `${title} ${none} ${protection()[key].join(" ")}`,
    view: () => (
      <>
        <div class="setting-title">{title}</div>
        <Show when={protection()[key].length > 0} fallback={<p class="panel-hint flush">{none}</p>}>
          <ul class="panel-list">
            <For each={protection()[key]}>
              {(address) => (
                <li>
                  <span>{address}</span>
                  <button onClick={() => changeProtection({ ...protection(), [key]: protection()[key].filter((a) => a !== address) })}>
                    {remove}
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </>
    ),
  });

  const select = (
    words: string,
    label: string,
    aside: string,
    value: () => string,
    options: [string, string][],
    change: (value: string) => void,
  ): Item => ({
    words: `${words} ${label} ${aside}`,
    view: () => (
      <label class="setting-line">
        <span>
          {label}
          <Show when={aside}>
            <span class="setting-aside">{aside}</span>
          </Show>
        </span>
        <select value={value()} onChange={(e) => change(e.currentTarget.value)}>
          <For each={options}>{([v, text]) => <option value={v}>{text}</option>}</For>
        </select>
      </label>
    ),
  });

  const look = (next: Partial<Appearance>) => void saveAppearance({ ...appearance(), ...next }).catch((e) => setNote(errorText(e)));

  const pages = createMemo<Page[]>(() => [
    {
      id: "general",
      title: "General",
      groups: [
        {
          title: "Notifications and the tray",
          items: [
            generalSwitch("notifyNewMail", "Show a notification when new mail arrives", "For mail arriving in an inbox while Outpost is running."),
            generalSwitch(
              "closeToTray",
              "Keep running in the tray when the window is closed",
              "Mail keeps arriving. Click the tray icon to open the window again; its menu has Quit.",
            ),
            generalSwitch("startMinimised", "Start minimised to the tray", "Has no effect on a desktop without a tray."),
          ],
        },
        {
          title: "Email links",
          items: [
            {
              words: "default mail app program email links mailto register handler",
              view: () => (
                <div class="setting-line">
                  <span>
                    Open email links with Outpost
                    <span class="setting-aside">
                      Clicking an email address on a web page or in another program then starts a new message here.
                    </span>
                  </span>
                  <button onClick={() => void core.chooseDefaultMailApp().then(setNote, (e) => setNote(errorText(e)))}>
                    Make Outpost the default
                  </button>
                </div>
              ),
            },
          ],
        },
        {
          title: "Message list",
          items: [
            generalSwitch(
              "conversations",
              "Show mail as conversations",
              "One row per conversation, which opens into a tree. The button at the top of the list switches for the time being without changing this.",
            ),
            select(
              "preview lines message list text rows",
              "Lines of each message's text shown in the list",
              "",
              () => String(general().previewLines),
              [
                ["0", "None"],
                ["1", "1"],
                ["2", "2"],
                ["3", "Up to 3"],
              ],
              (v) => changeGeneral({ ...general(), previewLines: Number(v) }),
            ),
          ],
        },
        {
          title: "Sending",
          items: [
            select(
              "undo send delay seconds wait before sending",
              "Wait before sending, so it can be undone",
              "An Undo button shows for this long after you press Send.",
              () => String(general().undoSendSeconds),
              [
                ["0", "Send at once"],
                ["5", "5 seconds"],
                ["10", "10 seconds"],
                ["20", "20 seconds"],
                ["30", "30 seconds"],
                ["60", "1 minute"],
              ],
              (v) => changeGeneral({ ...general(), undoSendSeconds: Number(v) }),
            ),
          ],
        },
      ],
    },
    {
      id: "appearance",
      title: "Appearance",
      groups: [
        {
          title: "Colours",
          items: [
            {
              words: "colour color scheme theme dark light paper slate high contrast mode night",
              view: () => (
                <>
                  <div class="setting-title">Colour scheme</div>
                  <div class="scheme-grid" role="radiogroup" aria-label="Colour scheme">
                    <For each={SCHEMES}>
                      {(s) => (
                        <button
                          type="button"
                          role="radio"
                          aria-checked={appearance().scheme === s.id}
                          class="scheme"
                          classList={{ chosen: appearance().scheme === s.id }}
                          onClick={() => look({ scheme: s.id })}
                        >
                          <span
                            class="scheme-swatch"
                            classList={{ split: s.id === "system" }}
                            style={{ "--a": s.swatch[0], "--b": s.swatch[1], "--c": s.swatch[2] }}
                          >
                            <i />
                            <i />
                            <i />
                          </span>
                          <span class="scheme-name">{s.name}</span>
                          <Show when={s.hint}>
                            <span class="setting-aside">{s.hint}</span>
                          </Show>
                        </button>
                      )}
                    </For>
                  </div>
                  <p class="panel-hint flush">
                    Formatted messages are always shown on a light page, as their senders laid them out. Plain messages
                    follow the scheme.
                  </p>
                </>
              ),
            },
            {
              words: "accent colour color highlight selection blue teal green purple rose orange graphite",
              view: () => (
                <div class="setting-line">
                  <span>
                    Accent colour
                    <span class="setting-aside">Used for the selection, buttons and links.</span>
                  </span>
                  <span class="accents" role="radiogroup" aria-label="Accent colour">
                    <For each={ACCENTS}>
                      {(a) => (
                        <button
                          type="button"
                          role="radio"
                          aria-checked={appearance().accent === a.id}
                          aria-label={a.name}
                          title={a.name}
                          class="accent-dot"
                          classList={{ chosen: appearance().accent === a.id }}
                          style={{ "--dot": a.color }}
                          onClick={() => look({ accent: a.id })}
                        />
                      )}
                    </For>
                  </span>
                </div>
              ),
            },
          ],
        },
        {
          title: "The window",
          items: [
            {
              words: "font interface window typeface menus list",
              view: () => (
                <FontPick
                  label="Font"
                  aside="For the folders, the message list, menus and buttons."
                  standard="Inter (built in)"
                  value={appearance().font}
                  change={(font) => look({ font })}
                />
              ),
            },
            select(
              "text size font size zoom bigger smaller larger scale",
              "Size",
              "Makes everything in the window larger or smaller together.",
              () => String(appearance().textSize),
              TEXT_SIZES.map((n): [string, string] => [String(n), n === 100 ? "100% (normal)" : `${n}%`]),
              (v) => look({ textSize: Number(v) }),
            ),
          ],
        },
        {
          title: "Messages",
          items: [
            {
              words: "font reading message text typeface",
              view: () => (
                <FontPick
                  label="Font for reading"
                  aside="Used where a message does not choose its own font."
                  standard="The system's"
                  value={appearance().readingFont}
                  change={(readingFont) => look({ readingFont })}
                />
              ),
            },
            select(
              "reading message text size font size",
              "Text size in messages",
              "For reading and for writing.",
              () => String(appearance().readingSize),
              READING_SIZES.map((n): [string, string] => [String(n), n === 14 ? "14 (normal)" : String(n)]),
              (v) => look({ readingSize: Number(v) }),
            ),
            {
              words: "font writing composing message text typeface",
              view: () => (
                <FontPick
                  label="Font for writing"
                  aside="What you see while writing. Messages are sent as plain text, so the recipient's own font is used at their end."
                  standard="Same as for reading"
                  value={appearance().writingFont}
                  change={(writingFont) => look({ writingFont })}
                />
              ),
            },
            {
              words: "sample preview font reading",
              view: () => (
                <p class="reading-sample" aria-label="Sample of message text">
                  This is how the text of a message looks. The quick brown fox jumps over the lazy dog: 0123456789.
                </p>
              ),
            },
          ],
        },
        {
          items: [
            {
              words: "reset appearance defaults colour font size",
              view: () => (
                <div class="setting-line">
                  <span>Put the appearance back as it came</span>
                  <button onClick={() => look(DEFAULT_APPEARANCE)}>Reset appearance</button>
                </div>
              ),
            },
          ],
        },
      ],
    },
    {
      id: "protection",
      title: "Protection",
      groups: [
        {
          title: "Warnings and checks",
          items: PROTECTION.map(
            (s): Item => ({
              words: `${s.label} ${s.hint} protection`,
              view: () => (
                <Switch
                  on={protection()[s.key]}
                  label={s.label}
                  hint={s.hint}
                  change={(on) => changeProtection({ ...protection(), [s.key]: on })}
                />
              ),
            }),
          ),
        },
        {
          title: "Senders",
          items: [
            list("allowImagesFrom", "Senders whose pictures are loaded", "None. Add one from a message that has pictures.", "Remove"),
            list("blockedSenders", "Blocked senders", "None. A blocked sender's mail goes to the junk folder.", "Unblock"),
          ],
        },
        {
          title: "Settings password",
          items: [
            {
              words: "settings password require protect lock",
              view: () => (
                <form onSubmit={setPassword}>
                  <p class="panel-hint flush">
                    With a password set, no setting can be changed (other than how Outpost looks), no account removed
                    and no warning overridden without it. It stops someone at the keyboard being talked into switching
                    protection off. It is not a defence against someone who can change files on this computer.
                  </p>
                  <Show when={passwordSet()}>
                    <label>
                      Current password
                      <input type="password" autocomplete="off" value={current()} onInput={(e) => setCurrent(e.currentTarget.value)} />
                    </label>
                  </Show>
                  <div class="panel-row">
                    <label class="grow">
                      {passwordSet() ? "New password (empty removes it)" : "Password"}
                      <input type="password" autocomplete="new-password" value={next()} onInput={(e) => setNext(e.currentTarget.value)} />
                    </label>
                    <label class="grow">
                      The same again
                      <input type="password" autocomplete="new-password" value={repeat()} onInput={(e) => setRepeat(e.currentTarget.value)} />
                    </label>
                  </div>
                  <Show when={passwordNote()}>
                    <p class="panel-note" role="status">
                      {passwordNote()}
                    </p>
                  </Show>
                  <button type="submit" disabled={!passwordSet() && !next()}>
                    {passwordSet() ? (next() ? "Change password" : "Remove password") : "Set password"}
                  </button>
                </form>
              ),
            },
          ],
        },
      ],
    },
    ...props.accounts
      .filter((a) => a.host)
      .map((account): Page => {
        const items = accountItems(account, props.onAccountsChanged, setNote, props.onPassword);
        return {
          id: `mail:${account.id}`,
          title: account.username,
          under: "Mail accounts",
          groups: [
            { title: "Incoming mail", items: items.slice(0, 2) },
            { title: "Sending and identity", items: items.slice(2, 3) },
            { title: "Signature", items: items.slice(3, 4) },
            { title: "Sender checks", items: items.slice(4, 5) },
            { title: "Remove", items: items.slice(5) },
          ],
        };
      }),
    {
      id: "mail:add",
      title: "Add a mail account",
      under: "Mail accounts",
      groups: [
        {
          items: [
            {
              words: "add mail account new mailbox imap",
              view: () => (
                <div class="setting-line">
                  <span>
                    Add a mail account
                    <span class="setting-aside">Any mailbox that offers IMAP. Gmail and similar need an app password.</span>
                  </span>
                  <button onClick={props.onAddAccount}>Add account</button>
                </div>
              ),
            },
          ],
        },
      ],
    },
    ...davAccounts().map((account): Page => {
      const items = davItems(account, setNote);
      const first = items.findIndex((i) => i.words.includes(" colour color show sync"));
      const last = items.length - 1;
      const split = first < 0 ? last : first;
      return {
        id: `dav:${account.id}`,
        title: account.username,
        under: "Calendar and contacts",
        groups: [
          { title: "Connection", items: items.slice(0, split) },
          { title: "Calendars, task lists and address books", items: items.slice(split, last) },
          { title: "Remove", items: items.slice(last) },
        ].filter((g) => g.items.length > 0),
      };
    }),
    {
      id: "dav:add",
      title: "Add an account",
      under: "Calendar and contacts",
      groups: [
        { items: [{ words: "add calendar contacts account caldav carddav nextcloud tasks address book", view: () => <AddDav /> }] },
      ],
    },
    {
      id: "shortcuts",
      title: "Keyboard shortcuts",
      groups: SHORTCUT_GROUPS.map((g) => ({
        title: g.title,
        tight: true,
        items: SHORTCUTS.filter((s) => g.ids.includes(s.id)).map(
          (s): Item => ({
            words: `${s.label} ${s.keys} keyboard shortcut key`,
            view: () => (
              <div class="setting-line shortcut">
                <span>{s.label}</span>
                <kbd>{s.keys}</kbd>
              </div>
            ),
          }),
        ),
      })),
    },
    {
      id: "about",
      title: "About and updates",
      groups: [
        {
          title: "Updates",
          items: [
            generalSwitch(
              "checkForUpdates",
              "Check for a new version once a day",
              "Asks github.com for the latest release of Outpost. Nothing about you or your mail is sent; GitHub sees your internet address, as any website you visit does. A new version is announced, never installed for you.",
            ),
            {
              words: "update check now version new release download",
              view: () => <UpdateLine version={props.coreVersion} />,
            },
          ],
        },
        {
          title: "About",
          items: [
            {
              words: "about version licence license outpost privacy",
              view: () => (
                <p class="panel-hint flush">
                  Outpost Mail {props.coreVersion}. Outpost's own code is released under the Unlicense. It has no
                  telemetry: it talks to your mail and calendar servers, to DNS when checking a signature, and to
                  github.com when checking for a new version.
                </p>
              ),
            },
            {
              words: "log file troubleshooting problem report diagnostics",
              view: () => (
                <div class="setting-line">
                  <span>
                    Log
                    <span class="setting-aside">
                      What Outpost did and what went wrong, kept on this computer for troubleshooting. It names
                      folders and servers; it never contains passwords or the text of mail.
                    </span>
                  </span>
                  <button onClick={() => void core.revealLog().catch((e) => setNote(errorText(e)))}>Show the log file</button>
                </div>
              ),
            },
          ],
        },
      ],
    },
  ]);

  const words = () => query().toLowerCase().split(/\s+/).filter(Boolean);
  const matches = (item: Item) => {
    const text = item.words.toLowerCase();
    return words().every((w) => text.includes(w));
  };
  const searching = () => words().length > 0;
  const [chosen, setChosen] = createSignal(props.page ?? "general");
  // "mail:" opens the first mail account's section, whichever that is.
  const page = () => pages().find((p) => p.id === chosen()) ?? pages().find((p) => p.id.startsWith(chosen())) ?? pages()[0]!;
  /** While searching: every page that has a match, cut down to its matches. */
  const found = createMemo(() =>
    pages()
      .map((p) => ({
        ...p,
        groups: p.groups.map((g) => ({ ...g, items: g.items.filter(matches) })).filter((g) => g.items.length > 0),
      }))
      .filter((p) => p.groups.length > 0),
  );
  const shown = () => (searching() ? found() : [page()]);
  const navGroups = createMemo(() => {
    const out: { heading: string; pages: Page[] }[] = [];
    for (const p of pages()) {
      const heading = p.under ?? "";
      const lastGroup = out[out.length - 1];
      if (lastGroup && lastGroup.heading === heading) lastGroup.pages.push(p);
      else out.push({ heading, pages: [p] });
    }
    return out;
  });
  let body: HTMLDivElement | undefined;

  return (
    <div class="settings">
      <header class="settings-head">
        <h1>Settings</h1>
        <input
          type="search"
          class="settings-search"
          placeholder="Search all settings"
          aria-label="Search all settings"
          ref={(el) => setTimeout(() => el.focus())}
          value={query()}
          onInput={(e) => setQuery(e.currentTarget.value)}
        />
        <button onClick={props.onClose} title="Close (Escape)">
          Close
        </button>
      </header>
      <div class="settings-main">
        <nav class="settings-nav" aria-label="Settings sections">
          <For each={navGroups()}>
            {(group) => (
              <>
                <Show when={group.heading}>
                  <div class="settings-nav-heading">{group.heading}</div>
                </Show>
                <For each={group.pages}>
                  {(p) => (
                    <button
                      classList={{
                        chosen: !searching() && page().id === p.id,
                        dim: searching() && !found().some((f) => f.id === p.id),
                      }}
                      aria-current={!searching() && page().id === p.id ? "page" : undefined}
                      title={p.title}
                      onClick={() => {
                        setQuery("");
                        setChosen(p.id);
                        body?.scrollTo(0, 0);
                      }}
                    >
                      {p.title}
                    </button>
                  )}
                </For>
              </>
            )}
          </For>
        </nav>
        <div class="settings-body panel" ref={body}>
          <Show when={passwordSet()}>
            <p class="panel-note">Changing a setting needs the settings password. How Outpost looks can be changed without it.</p>
          </Show>
          <Show when={note()}>
            <p class="panel-note" role="status">
              {note()}
            </p>
          </Show>
          <For each={shown()}>
            {(p) => (
              <section class="settings-page">
                <h2>{p.under ? `${p.under}: ${p.title}` : p.title}</h2>
                <For each={p.groups}>
                  {(g) => (
                    <div class="setting-group">
                      <Show when={g.title}>
                        <h3>{g.title}</h3>
                      </Show>
                      <div class="setting-card" classList={{ tight: g.tight }}>
                        <For each={g.items}>{(item) => <div class="setting">{item.view()}</div>}</For>
                      </div>
                    </div>
                  )}
                </For>
              </section>
            )}
          </For>
          <Show when={searching() && found().length === 0}>
            <p class="panel-note">No setting matches "{query()}".</p>
          </Show>
        </div>
      </div>
    </div>
  );
}

const SHORTCUT_GROUPS: { title: string; ids: string[] }[] = [
  { title: "Writing", ids: ["new", "reply", "replyAll", "forward", "send"] },
  { title: "Messages", ids: ["delete", "archive", "flag", "read", "unread", "selectAll", "up", "extend", "print"] },
  { title: "Getting around", ids: ["mail", "calendar", "people", "tasks", "search", "sync", "settings", "close"] },
];

/** Choosing a font: the ones this computer has, or any other by name. */
function FontPick(props: { label: string; aside: string; standard: string; value: string; change: (font: string) => void }) {
  const fonts = () => {
    const all = installedFonts();
    return props.value && !all.includes(props.value) ? [props.value, ...all] : all;
  };
  const [other, setOther] = createSignal<string>();
  const [missing, setMissing] = createSignal(false);
  return (
    <>
      <label class="setting-line">
        <span>
          {props.label}
          <span class="setting-aside">{props.aside}</span>
        </span>
        <select
          value={other() !== undefined ? "\u0000other" : props.value}
          style={{ "font-family": props.value ? `"${props.value}", var(--font)` : undefined }}
          onChange={(e) => {
            const value = e.currentTarget.value;
            setMissing(false);
            if (value === "\u0000other") setOther("");
            else {
              setOther(undefined);
              props.change(value);
            }
          }}
        >
          <option value="">{props.standard}</option>
          <For each={fonts()}>{(f) => <option value={f} style={{ "font-family": `"${f}"` }}>{f}</option>}</For>
          <option value={"\u0000other"}>Another font…</option>
        </select>
      </label>
      <Show when={other() !== undefined}>
        <form
          class="setting-line font-other"
          onSubmit={(e) => {
            e.preventDefault();
            const name = (other() ?? "").trim();
            if (!name) return;
            if (!fontInstalled(name)) {
              setMissing(true);
              return;
            }
            setOther(undefined);
            props.change(name);
          }}
        >
          <input
            type="text"
            class="grow"
            spellcheck={false}
            placeholder="The font's name, as other programs list it"
            aria-label={`${props.label}: name of another font`}
            value={other()}
            onInput={(e) => {
              setOther(e.currentTarget.value);
              setMissing(false);
            }}
          />
          <button type="submit">Use it</button>
        </form>
        <Show when={missing()}>
          <p class="panel-hint">This computer does not seem to have a font of that name.</p>
        </Show>
      </Show>
    </>
  );
}

/** What the last check for a new version found, and a button to check now. */
function UpdateLine(props: { version: string }) {
  const [checking, setChecking] = createSignal(false);
  const [problem, setProblem] = createSignal("");
  const check = () => {
    setChecking(true);
    setProblem("");
    void checkForUpdate(true)
      .catch((e) => setProblem(errorText(e)))
      .finally(() => setChecking(false));
  };
  return (
    <div class="setting-line">
      <span>
        <Show when={update()} fallback={<>This is version {props.version}.</>}>
          {(u) => (
            <>
              Version {u().version} is out; this is {props.version}.
            </>
          )}
        </Show>
        <span class="setting-aside">
          {problem()
            ? `Could not check: ${problem()}`
            : update()
              ? "Download the installer from the release page and run it; your mail and settings stay."
              : updateChecked()
                ? "No newer version has been released."
                : "Not checked yet."}
        </span>
      </span>
      <span class="setting-buttons">
        <Show when={update()}>{(u) => <button onClick={() => void core.openLink(u().url)}>Open the release page</button>}</Show>
        <button disabled={checking()} onClick={check}>
          {checking() ? "Checking…" : "Check now"}
        </button>
      </span>
    </div>
  );
}

/** Asks for the settings password when a guarded change is being made. */
export function PasswordAsk(props: { why: string; error: string; answer: (password: string | null) => void }) {
  const [password, setPassword] = createSignal("");
  return (
    <div class="ask-back">
      <form
        class="ask"
        role="dialog"
        aria-label="Settings password"
        onSubmit={(e) => {
          e.preventDefault();
          props.answer(password());
        }}
      >
        <p>{props.why}, enter the settings password.</p>
        <input
          type="password"
          autocomplete="off"
          ref={(el) => setTimeout(() => el.focus())}
          value={password()}
          onInput={(e) => setPassword(e.currentTarget.value)}
        />
        <Show when={props.error}>
          <p class="panel-error" role="alert">
            {props.error}
          </p>
        </Show>
        <div class="panel-actions">
          <button type="submit" class="primary">
            Continue
          </button>
          <button type="button" onClick={() => props.answer(null)}>
            Cancel
          </button>
        </div>
      </form>
    </div>
  );
}
