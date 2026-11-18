// The composer: writing a message. A panel in the reading area, so the
// folder tree and message list stay usable while writing.
//
// Plain text for now. An account that has never sent mail is asked first
// for the name to send under and its outgoing server.

import { createSignal, For, Show } from "solid-js";
import { core, type AccountInfo, type DraftContent, type SendingSettings } from "../core";
import { general } from "../core/protection";

export const EMPTY_DRAFT: DraftContent = {
  to: "",
  cc: "",
  bcc: "",
  subject: "",
  body: "",
  inReplyTo: null,
  references: [],
  attachments: [],
  replyToMessage: null,
  forwardOf: null,
  replaces: null,
};

/** How long a sent message waits in the outbox, where Undo can catch it. */

function errorText(e: unknown): string {
  return String((e as { message?: string })?.message ?? e);
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} kB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

async function toBase64(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  const step = 0x8000;
  for (let i = 0; i < bytes.length; i += step) {
    binary += String.fromCharCode(...bytes.subarray(i, i + step));
  }
  return btoa(binary);
}

function SendingSetup(props: { account: AccountInfo; onDone: () => void; onCancel: () => void }) {
  const guess = props.account.username.includes("@") ? props.account.username : "";
  const [name, setName] = createSignal("");
  const [address, setAddress] = createSignal(guess);
  const [host, setHost] = createSignal(props.account.host);
  const [security, setSecurity] = createSignal<SendingSettings["security"]>("tls");
  const [port, setPort] = createSignal("465");
  const [error, setError] = createSignal("");

  function submit(e: Event) {
    e.preventDefault();
    setError("");
    core
      .send({
        type: "setSending",
        account: props.account.id,
        sending: { name: name(), address: address(), host: host(), port: Number(port()), security: security() },
      })
      .then(props.onDone)
      .catch((e) => setError(errorText(e)));
  }

  return (
    <form class="panel" onSubmit={submit}>
      <h1>Set up sending</h1>
      <p class="panel-note">
        Before the first message goes out, Outpost needs to know who it is from and which server sends it. The
        outgoing server is signed in to with the same password as your mailbox.
      </p>
      <label>
        Your name, as recipients will see it
        <input type="text" required value={name()} onInput={(e) => setName(e.currentTarget.value)} />
      </label>
      <label>
        Email address to send from
        <input type="text" required value={address()} onInput={(e) => setAddress(e.currentTarget.value)} />
      </label>
      <div class="panel-row">
        <label class="grow">
          Outgoing mail server (SMTP)
          <input type="text" required value={host()} onInput={(e) => setHost(e.currentTarget.value)} />
        </label>
        <label class="narrow">
          Port
          <input type="text" inputmode="numeric" required value={port()} onInput={(e) => setPort(e.currentTarget.value)} />
        </label>
      </div>
      <label>
        Connection security
        <select
          value={security()}
          onChange={(e) => {
            const value = e.currentTarget.value as SendingSettings["security"];
            setSecurity(value);
            setPort(value === "tls" ? "465" : value === "starttls" ? "587" : "25");
          }}
        >
          <option value="tls">Encrypted from the start (TLS, usually port 465)</option>
          <option value="starttls">Upgraded to encrypted (STARTTLS, usually port 587)</option>
          <option value="plaintext">Not encrypted (only for a server on this computer)</option>
        </select>
      </label>
      <Show when={error()}>
        <p class="panel-error" role="alert">
          {error()}
        </p>
      </Show>
      <div class="panel-actions">
        <button type="submit" class="primary">
          Save and continue
        </button>
        <button type="button" onClick={props.onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}

/**
 * The text a message starts with: the account's signature goes under what
 * the user will write, and above anything quoted. A draft being reopened is
 * left exactly as it was saved.
 */
function withSignature(draft: DraftContent, signature: string): string {
  if (!signature.trim() || draft.replaces !== null) return draft.body;
  const block = `-- \n${signature.trim()}`;
  if (draft.body.includes(block)) return draft.body;
  return draft.body.trim() ? `\n\n${block}\n\n${draft.body.replace(/^\n+/, "")}` : `\n\n${block}\n`;
}

export function Composer(props: {
  account: AccountInfo;
  initial: DraftContent;
  /** The message was put in the outbox; `id` is its row there. */
  onSent: (id: number, subject: string) => void;
  onClose: (note?: string) => void;
  onSendingSet: () => void;
}) {
  const [to, setTo] = createSignal(props.initial.to);
  const [cc, setCc] = createSignal(props.initial.cc);
  const [bcc, setBcc] = createSignal(props.initial.bcc);
  const [subject, setSubject] = createSignal(props.initial.subject);
  const [body, setBody] = createSignal(withSignature(props.initial, props.account.signature));
  const [attachments, setAttachments] = createSignal(props.initial.attachments);
  const [showCopies, setShowCopies] = createSignal(Boolean(props.initial.cc || props.initial.bcc));
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  let files!: HTMLInputElement;

  const draft = (): DraftContent => ({
    ...props.initial,
    to: to(),
    cc: cc(),
    bcc: bcc(),
    subject: subject(),
    body: body(),
    attachments: attachments(),
  });

  async function addFiles() {
    const chosen = [...(files.files ?? [])];
    files.value = "";
    try {
      const added = await Promise.all(
        chosen.map(async (file) => ({ name: file.name, mimeType: file.type, dataBase64: await toBase64(file) })),
      );
      setAttachments((all) => [...all, ...added]);
    } catch (e) {
      setError(`Could not read the file: ${errorText(e)}`);
    }
  }

  function send(e: Event) {
    e.preventDefault();
    if (busy()) return;
    if (!subject().trim() && !window.confirm("Send this message without a subject?")) return;
    setBusy(true);
    setError("");
    core
      .send({ type: "sendMessage", account: props.account.id, draft: draft(), delaySeconds: general().undoSendSeconds })
      .then((r) => props.onSent(r.id, subject()))
      .catch((e) => {
        setBusy(false);
        setError(errorText(e));
      });
  }

  function saveDraft() {
    if (busy()) return;
    setBusy(true);
    setError("");
    core
      .send({ type: "saveDraft", account: props.account.id, draft: draft() })
      .then(() => props.onClose("Saved to Drafts"))
      .catch((e) => {
        setBusy(false);
        setError(errorText(e));
      });
  }

  function discard() {
    const written = to() || subject() || body().trim() || attachments().length > 0;
    if (written && !window.confirm("Discard this message? What you have written will be lost.")) return;
    props.onClose();
  }

  return (
    <Show
      when={props.account.sending}
      fallback={<SendingSetup account={props.account} onDone={props.onSendingSet} onCancel={() => props.onClose()} />}
    >
      {(sending) => (
        <form
          class="composer"
          onSubmit={send}
          onKeyDown={(e) => {
            if (e.ctrlKey && e.key === "Enter") send(e);
          }}
        >
          <div class="composer-from">
            From {sending().name ? `${sending().name} <${sending().address}>` : sending().address}
          </div>
          <label class="composer-line">
            <span>To</span>
            <input type="text" value={to()} onInput={(e) => setTo(e.currentTarget.value)} autofocus={!props.initial.to} />
            <Show when={!showCopies()}>
              <button type="button" class="inline" onClick={() => setShowCopies(true)}>
                Cc / Bcc
              </button>
            </Show>
          </label>
          <Show when={showCopies()}>
            <label class="composer-line">
              <span>Cc</span>
              <input type="text" value={cc()} onInput={(e) => setCc(e.currentTarget.value)} />
            </label>
            <label class="composer-line">
              <span>Bcc</span>
              <input type="text" value={bcc()} onInput={(e) => setBcc(e.currentTarget.value)} />
            </label>
          </Show>
          <label class="composer-line">
            <span>Subject</span>
            <input type="text" value={subject()} onInput={(e) => setSubject(e.currentTarget.value)} />
          </label>
          <textarea
            class="composer-body"
            aria-label="Message"
            value={body()}
            onInput={(e) => setBody(e.currentTarget.value)}
            ref={(el) => {
              // A reply starts with the cursor above the quoted text.
              if (props.initial.to) queueMicrotask(() => (el.focus(), el.setSelectionRange(0, 0)));
            }}
          />
          <Show when={props.initial.forwardOf !== null}>
            <p class="composer-note">Attachments of the forwarded message are included.</p>
          </Show>
          <Show when={attachments().length > 0}>
            <ul class="attachments">
              <For each={attachments()}>
                {(a, index) => (
                  <li>
                    <span class="attachment-name">{a.name}</span>
                    <span class="attachment-size">{formatSize(Math.floor((a.dataBase64.length * 3) / 4))}</span>
                    <button type="button" onClick={() => setAttachments((all) => all.filter((_, i) => i !== index()))}>
                      Remove
                    </button>
                  </li>
                )}
              </For>
            </ul>
          </Show>
          <Show when={error()}>
            <p class="panel-error" role="alert">
              {error()}
            </p>
          </Show>
          <div class="panel-actions">
            <button type="submit" class="primary" disabled={busy()}>
              Send
            </button>
            <button type="button" onClick={() => files.click()} disabled={busy()}>
              Attach file
            </button>
            <button type="button" onClick={saveDraft} disabled={busy()}>
              Save draft
            </button>
            <button type="button" onClick={discard} disabled={busy()}>
              Discard
            </button>
            <input type="file" multiple hidden ref={files} onChange={addFiles} />
          </div>
        </form>
      )}
    </Show>
  );
}
