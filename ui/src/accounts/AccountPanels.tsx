// Adding an account, and supplying a password for one that needs it.
//
// Both are panels in the reading area, not dialogs: nothing here blocks the
// rest of the window.

import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { core, type AccountInfo } from "../core";

function errorText(e: unknown): string {
  return String((e as { message?: string })?.message ?? e);
}

export function AddAccount(props: { onDone: (account: string) => void; onCancel: () => void }) {
  const [username, setUsername] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [host, setHost] = createSignal("");
  const [port, setPort] = createSignal("993");
  const [encrypted, setEncrypted] = createSignal(true);
  const [remember, setRemember] = createSignal(true);
  const [checking, setChecking] = createSignal(false);
  const [error, setError] = createSignal("");
  let hostEdited = false;

  onMount(() => {
    const stop = core.onEvent((event) => {
      if (!checking()) return;
      if (event.type === "accountAdded") {
        setChecking(false);
        props.onDone(event.account);
      } else if (event.type === "accountSetupFailed") {
        setChecking(false);
        setError(event.error);
      }
    });
    onCleanup(stop);
  });

  // A first guess at the server, from the address. Most hosts use this form.
  function guessHost() {
    const domain = username().split("@")[1]?.trim();
    if (domain && !hostEdited) setHost(`mail.${domain}`);
  }

  function submit(e: Event) {
    e.preventDefault();
    setError("");
    const portNumber = Number(port());
    if (!Number.isInteger(portNumber) || portNumber < 1 || portNumber > 65535) {
      setError("The port must be a number between 1 and 65535.");
      return;
    }
    setChecking(true);
    core
      .send({
        type: "addAccount",
        host: host(),
        port: portNumber,
        encrypted: encrypted(),
        username: username(),
        password: password(),
        remember: remember(),
      })
      .catch((e) => {
        setChecking(false);
        setError(errorText(e));
      });
  }

  return (
    <form class="panel" onSubmit={submit}>
      <h1>Add a mail account</h1>
      <p class="panel-note">
        Outpost signs in to check these details before saving anything. It reads your mail and does not change
        anything on the server.
      </p>
      <label>
        Email address or user name
        <input
          type="text"
          autocomplete="username"
          required
          value={username()}
          onInput={(e) => setUsername(e.currentTarget.value)}
          onBlur={guessHost}
        />
      </label>
      <label>
        Password
        <input
          type="password"
          autocomplete="current-password"
          required
          value={password()}
          onInput={(e) => setPassword(e.currentTarget.value)}
        />
      </label>
      <div class="panel-row">
        <label class="grow">
          Incoming mail server (IMAP)
          <input
            type="text"
            required
            placeholder="mail.example.com"
            value={host()}
            onInput={(e) => {
              hostEdited = true;
              setHost(e.currentTarget.value);
            }}
          />
        </label>
        <label class="narrow">
          Port
          <input type="text" inputmode="numeric" required value={port()} onInput={(e) => setPort(e.currentTarget.value)} />
        </label>
      </div>
      <label class="check">
        <input
          type="checkbox"
          checked={encrypted()}
          onChange={(e) => {
            setEncrypted(e.currentTarget.checked);
            if (port() === "993" || port() === "143") setPort(e.currentTarget.checked ? "993" : "143");
          }}
        />
        Encrypted connection (TLS)
      </label>
      <Show when={!encrypted()}>
        <p class="panel-warning">
          Without encryption your password and mail cross the network readable by anyone in between. Only use this for
          a server on your own machine.
        </p>
      </Show>
      <label class="check">
        <input type="checkbox" checked={remember()} onChange={(e) => setRemember(e.currentTarget.checked)} />
        Remember the password on this computer
      </label>
      <p class="panel-hint">
        It is kept in the system's password store, never in a file. If this system has none, Outpost asks again each
        time it starts.
      </p>
      <Show when={error()}>
        <p class="panel-error" role="alert">
          Could not add the account: {error()}
        </p>
      </Show>
      <div class="panel-actions">
        <button type="submit" class="primary" disabled={checking()}>
          {checking() ? "Checking…" : "Sign in and add"}
        </button>
        <button type="button" onClick={props.onCancel} disabled={checking()}>
          Cancel
        </button>
      </div>
    </form>
  );
}

export function PasswordPrompt(props: { account: AccountInfo; onDone: () => void; onCancel: () => void }) {
  const [password, setPassword] = createSignal("");
  const [remember, setRemember] = createSignal(true);
  const [error, setError] = createSignal("");

  function submit(e: Event) {
    e.preventDefault();
    core
      .send({ type: "providePassword", account: props.account.id, password: password(), remember: remember() })
      .then(props.onDone)
      .catch((e) => setError(errorText(e)));
  }

  return (
    <form class="panel" onSubmit={submit}>
      <h1>Password for {props.account.username}</h1>
      <p class="panel-note">
        <Show
          when={props.account.status.state === "signInFailed"}
          fallback={<>Outpost has no password stored for this account on {props.account.host}.</>}
        >
          The server refused the password: {props.account.status.detail}
        </Show>{" "}
        Mail already downloaded can still be read.
      </p>
      <label>
        Password
        <input
          type="password"
          autocomplete="current-password"
          required
          autofocus
          value={password()}
          onInput={(e) => setPassword(e.currentTarget.value)}
        />
      </label>
      <label class="check">
        <input type="checkbox" checked={remember()} onChange={(e) => setRemember(e.currentTarget.checked)} />
        Remember the password on this computer
      </label>
      <Show when={error()}>
        <p class="panel-error" role="alert">
          {error()}
        </p>
      </Show>
      <div class="panel-actions">
        <button type="submit" class="primary">
          Sign in
        </button>
        <button type="button" onClick={props.onCancel}>
          Not now
        </button>
      </div>
    </form>
  );
}
