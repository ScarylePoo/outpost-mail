// The protection settings: each its own switch, with a line saying what it
// does. Milestone 8 folds this into the settings window; the switches and
// the password are complete here.

import { createSignal, For, Show } from "solid-js";
import { core, type AccountInfo, type Protection } from "../core";
import { passwordSet, protection, savePassword, saveProtection } from "../core/protection";

type Switch = Exclude<keyof Protection, "allowImagesFrom" | "blockedSenders">;

const SWITCHES: { key: Switch; label: string; hint: string }[] = [
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
  {
    key: "firstTimeBanner",
    label: "Say when a message is the first from its sender",
    hint: "",
  },
  {
    key: "lookalikeWarning",
    label: "Warn when a sender's domain resembles one you get mail from",
    hint: "Such as a bank's name with one letter changed.",
  },
  {
    key: "replyWarning",
    label: "Warn when replies would go to a different domain than the sender's",
    hint: "",
  },
  {
    key: "showLinkDestination",
    label: "Always show where a link really goes before opening it",
    hint: "When off, clicking a link opens it straight away.",
  },
  {
    key: "recheckSignatures",
    label: "Check signatures again on this computer",
    hint: "Looks up the signer's key through this computer's DNS, as following a link to their site would.",
  },
];

function errorText(e: unknown): string {
  return String((e as { message?: string })?.message ?? e);
}

export function ProtectionPanel(props: { accounts: AccountInfo[]; onAccountsChanged: () => void; onClose: () => void }) {
  const [error, setError] = createSignal("");
  const [current, setCurrent] = createSignal("");
  const [next, setNext] = createSignal("");
  const [repeat, setRepeat] = createSignal("");
  const [passwordNote, setPasswordNote] = createSignal("");

  async function change(next: Protection) {
    setError("");
    try {
      await saveProtection("To change the protection settings", next);
    } catch (e) {
      setError(errorText(e));
    }
  }

  const without = (list: "allowImagesFrom" | "blockedSenders", address: string) =>
    change({ ...protection(), [list]: protection()[list].filter((a) => a !== address) });

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

  function setServer(account: AccountInfo, server: string) {
    core
      .send({ type: "setCheckingServer", account: account.id, server })
      .then(props.onAccountsChanged)
      .catch((e) => setError(errorText(e)));
  }

  return (
    <div class="panel wide">
      <h1>Protection</h1>
      <p class="panel-note">
        Each of these is its own switch.
        <Show when={passwordSet()}> Changing any of them needs the settings password.</Show>
      </p>
      <Show when={error()}>
        <p class="panel-error" role="alert">
          {error()}
        </p>
      </Show>

      <For each={SWITCHES}>
        {(s) => (
          <>
            <label class="check">
              <input
                type="checkbox"
                checked={protection()[s.key]}
                onChange={(e) => {
                  const wanted = e.currentTarget.checked;
                  // Shown as it is until the change has really been made.
                  e.currentTarget.checked = protection()[s.key];
                  void change({ ...protection(), [s.key]: wanted });
                }}
              />
              {s.label}
            </label>
            <Show when={s.hint}>
              <p class="panel-hint">{s.hint}</p>
            </Show>
          </>
        )}
      </For>

      <h2>Senders whose pictures are loaded</h2>
      <Show when={protection().allowImagesFrom.length > 0} fallback={<p class="panel-hint flush">None. Add one from a message that has pictures.</p>}>
        <ul class="panel-list">
          <For each={protection().allowImagesFrom}>
            {(address) => (
              <li>
                <span>{address}</span>
                <button onClick={() => void without("allowImagesFrom", address)}>Remove</button>
              </li>
            )}
          </For>
        </ul>
      </Show>

      <h2>Blocked senders</h2>
      <Show when={protection().blockedSenders.length > 0} fallback={<p class="panel-hint flush">None. Their mail would go to the junk folder.</p>}>
        <ul class="panel-list">
          <For each={protection().blockedSenders}>
            {(address) => (
              <li>
                <span>{address}</span>
                <button onClick={() => void without("blockedSenders", address)}>Unblock</button>
              </li>
            )}
          </For>
        </ul>
      </Show>

      <h2>Whose sender checks are believed</h2>
      <p class="panel-hint flush">
        Your own mail server writes the result of its checks into each message. Anyone can write the same thing into a
        message they send, so Outpost believes only the server named here, and only what it wrote at the very top.
      </p>
      <Show when={props.accounts.length > 0} fallback={<p class="panel-hint flush">No accounts yet.</p>}>
        <For each={props.accounts}>
          {(account) => {
            const [server, setServerText] = createSignal(account.checkingServer);
            return (
              <form
                class="panel-server"
                onSubmit={(e) => {
                  e.preventDefault();
                  setServer(account, server());
                }}
              >
                <label>
                  {account.name || account.username}
                  <input
                    type="text"
                    spellcheck={false}
                    placeholder="for example mx.yourhost.com"
                    value={server()}
                    onInput={(e) => setServerText(e.currentTarget.value)}
                  />
                </label>
                <button type="submit">
                  {account.checkingServerConfirmed && server() === account.checkingServer ? "Confirmed" : "Confirm"}
                </button>
                <Show when={!account.checkingServerConfirmed}>
                  <p class="panel-hint flush">
                    <Show when={account.checkingServer} fallback="Not found yet: none of this account's recent mail carries check results.">
                      Found on most of this account's recent mail. Confirm it if it is your mail provider's server.
                    </Show>
                  </p>
                </Show>
              </form>
            );
          }}
        </For>
      </Show>

      <h2>Settings password</h2>
      <p class="panel-hint flush">
        With a password set, these settings cannot be changed, and a warning cannot be overridden, without it. It stops
        someone at the keyboard being talked into switching protection off. It is not a defence against someone who
        can change files on this computer.
      </p>
      <form onSubmit={setPassword}>
        <Show when={passwordSet()}>
          <label>
            Current password
            <input type="password" autocomplete="off" value={current()} onInput={(e) => setCurrent(e.currentTarget.value)} />
          </label>
        </Show>
        <label>
          {passwordSet() ? "New password (leave empty to remove it)" : "Password"}
          <input type="password" autocomplete="new-password" value={next()} onInput={(e) => setNext(e.currentTarget.value)} />
        </label>
        <label>
          The same again
          <input type="password" autocomplete="new-password" value={repeat()} onInput={(e) => setRepeat(e.currentTarget.value)} />
        </label>
        <Show when={passwordNote()}>
          <p class="panel-note" role="status">
            {passwordNote()}
          </p>
        </Show>
        <div class="panel-actions">
          <button type="submit" disabled={!passwordSet() && !next()}>
            {passwordSet() ? (next() ? "Change password" : "Remove password") : "Set password"}
          </button>
          <button type="button" onClick={props.onClose}>
            Close
          </button>
        </div>
      </form>
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
          autofocus
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
