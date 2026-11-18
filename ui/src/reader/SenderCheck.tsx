// Who a message is really from: the check mark, the neutral label, or the
// failed-message banner, with the detail behind each.
//
// The rule this file must never break: a green check appears only for the
// state "verified", which the core gives only when the account's own server
// recorded a pass tied to the domain in the From line.

import { createEffect, createSignal, For, on, Show } from "solid-js";
import { core, type FolderId, type MessageView, type SignatureLine } from "../core";
import { messageActions, protection, saveProtection, withPassword } from "../core/protection";

function domainOf(address: string): string {
  return address.split("@")[1]?.toLowerCase() ?? "";
}

export function SenderCheckPanel(props: {
  folder: FolderId;
  view: MessageView;
  /** Called when the message should be read again (it was trusted, say). */
  onChanged: () => void;
}) {
  const check = () => props.view.senderCheck;
  const failed = () => check().state === "failed";
  const [open, setOpen] = createSignal(false);
  const [signatures, setSignatures] = createSignal<SignatureLine[]>();
  /** Outpost's own check of the signature vouches for the sender. */
  const [vouched, setVouched] = createSignal(false);
  /** Only this may put a green check on screen. */
  const verified = () => check().state === "verified" || (vouched() && !failed());
  const [rechecking, setRechecking] = createSignal(false);
  const [recheckError, setRecheckError] = createSignal("");
  const [note, setNote] = createSignal("");
  let request = 0;

  function recheck() {
    const mine = ++request;
    setRechecking(true);
    setRecheckError("");
    core
      .send({ type: "recheckSignature", folder: props.folder, id: props.view.id })
      .then((r) => {
        if (mine !== request) return;
        setSignatures(r.signatures);
        setVouched(r.verified);
      })
      .catch((e) => mine === request && setRecheckError(String(e?.message ?? e)))
      .finally(() => mine === request && setRechecking(false));
  }

  // A new message: forget the last one's details. A failed message has its
  // signature looked at again straight away; that is part of its view.
  createEffect(
    on(
      () => props.view.id,
      () => {
        request++;
        setOpen(false);
        setSignatures(undefined);
        setVouched(false);
        setRechecking(false);
        setRecheckError("");
        setNote("");
        // So has a message the server did not verify: a valid signature by
        // the sender's own domain verifies it without the server's word.
        if (check().state !== "verified" && props.view.downloaded && protection().recheckSignatures) recheck();
      },
    ),
  );

  function showDetails() {
    const next = !open();
    setOpen(next);
    if (next && signatures() === undefined && !rechecking() && protection().recheckSignatures && check().checks.length > 0) {
      recheck();
    }
  }

  async function trust(trusted: boolean) {
    try {
      const done = await withPassword("To trust a message that failed its sender checks", (password) =>
        core.send({ type: "trustMessage", folder: props.folder, id: props.view.id, trusted, password }),
      );
      if (done) props.onChanged();
    } catch (e) {
      setNote(String((e as Error)?.message ?? e));
    }
  }

  async function block() {
    const address = props.view.senderAddress.toLowerCase();
    if (!address) return;
    const p = protection();
    try {
      const done = await saveProtection(`To block ${address}`, {
        ...p,
        blockedSenders: [...p.blockedSenders.filter((a) => a !== address), address],
      });
      if (done) setNote(`Mail from ${address} will go to the junk folder from now on.`);
    } catch (e) {
      setNote(String((e as Error)?.message ?? e));
    }
  }

  const label = () => {
    if (verified()) return "Verified sender";
    switch (check().state) {
      case "unverified":
        return "Sender not verified";
      case "failed":
        return "Failed sender checks";
      default:
        return "Sender not checked";
    }
  };

  const explanation = () => {
    const from = domainOf(props.view.senderAddress);
    if (vouched() && check().state !== "verified" && !failed()) {
      return `Your mail server recorded nothing that ties this message to ${from}, but the message is signed by ${from} and Outpost checked that signature itself: it is valid.`;
    }
    switch (check().state) {
      case "verified":
        return `Your mail server checked this message and it really comes from ${from || "the address shown"}.`;
      case "unverified":
        return check().ambiguousFrom
          ? "This message names more than one sender, so it cannot be tied to any one of them."
          : `Your mail server checked this message, but nothing ties it to ${from || "the address shown"}. That is common for mailing lists and forwarded mail, and also how forgeries look.`;
      case "failed":
        return "";
      default:
        return check().server
          ? "Your mail server recorded no usable checks on this message."
          : "No sender checks from your own mail server were found on this message. That is normal for mail you sent and for drafts.";
    }
  };

  const Details = () => (
    <div class="check-details">
      <Show when={explanation()}>
        <p>{explanation()}</p>
      </Show>
      <ul class="check-lines">
        <For each={check().checks}>
          {(line) => (
            <li classList={{ pass: line.outcome === "pass" && line.aligned, fail: line.outcome === "fail" }}>
              <span class="check-name">{line.name}</span>
              <span class="check-word">{line.word}</span>
              <span class="check-sentence">{line.sentence}</span>
            </li>
          )}
        </For>
      </ul>
      <Show when={check().server}>
        <p class="check-source">
          Checked by {check().server}, your mail server.
          <Show when={check().checks.some((c) => c.name === "SPF")}> SPF is that server's result; a mail program cannot repeat it.</Show>
        </p>
      </Show>
      <Show when={rechecking()}>
        <p class="check-source">Checking the signature again on this computer…</p>
      </Show>
      <For each={signatures() ?? []}>
        {(s) => (
          <p class="check-source" classList={{ mismatch: s.outcome === "mismatch" }}>
            {s.sentence}
          </p>
        )}
      </For>
      <Show when={signatures()?.length === 0}>
        <p class="check-source">The message carries no signature that could be checked again here.</p>
      </Show>
      <Show when={recheckError()}>
        <p class="check-source">The signature was not checked again here: {recheckError()}</p>
      </Show>
      <Show when={!protection().recheckSignatures}>
        <p class="check-source">Checking signatures again on this computer is switched off in the protection settings.</p>
      </Show>
    </div>
  );

  return (
    <div class="sender-check">
      <Show
        when={failed()}
        fallback={
          <>
            <button
              class="check-label"
              classList={{ verified: verified() }}
              aria-expanded={open()}
              onClick={showDetails}
              title="Show what was checked"
            >
              <Show when={verified()}>
                <span class="check-mark" aria-hidden="true">
                  ✓
                </span>
              </Show>
              {label()}
            </button>
            <Show when={open()}>
              <Details />
            </Show>
          </>
        }
      >
        <div class="failed" role="alert">
          <h2>This message failed its sender checks</h2>
          <p>
            It may not be from who it says.
            <Show
              when={props.view.restricted}
              fallback={
                <Show when={props.view.trusted} fallback={<> Its links and pictures are shown as usual, because that protection is switched off.</>}>
                  {" "}
                  You chose to trust it, so its links and attachments work.
                </Show>
              }
            >
              {" "}
              Its links and remote pictures are off, and its attachments cannot be saved.
            </Show>
          </p>
          <dl class="failed-who">
            <dt>Name shown</dt>
            <dd>{props.view.senderName || "(none)"}</dd>
            <dt>Actually sent from</dt>
            <dd classList={{ odd: check().nameSeenFrom !== "" || check().lookalikeOf !== "" }}>
              {props.view.senderAddress || "(no address)"}
              <Show when={check().nameSeenFrom}>
                <span class="failed-aside"> — earlier mail from this name came from {check().nameSeenFrom}</span>
              </Show>
            </dd>
            <dt>Replies would go to</dt>
            <dd classList={{ odd: check().replyElsewhere }}>{check().replyTo || props.view.senderAddress || "(no address)"}</dd>
          </dl>
          <Details />
          <div class="failed-actions">
            <button onClick={() => messageActions.remove?.()}>Delete</button>
            <Show when={messageActions.hasJunk?.()}>
              <button onClick={() => messageActions.moveToJunk?.()}>Move to spam</button>
            </Show>
            <button onClick={block}>Block sender</button>
            <span class="failed-gap" />
            <Show
              when={props.view.trusted}
              fallback={
                <Show when={props.view.restricted}>
                  <button class="inline low-key" onClick={() => void trust(true)}>
                    I trust this message; turn its links on
                  </button>
                </Show>
              }
            >
              <button class="inline low-key" onClick={() => void trust(false)}>
                Stop trusting this message
              </button>
            </Show>
          </div>
        </div>
      </Show>

      <Show when={note()}>
        <div class="message-note">{note()}</div>
      </Show>

      {/* Outpost's own warnings. On a failed message the banner covers the
          name and the reply address; these are for the rest. */}
      <Show when={check().lookalikeOf}>
        <div class="warning">
          The sender's address is at <strong>{domainOf(props.view.senderAddress)}</strong>, which looks like{" "}
          <strong>{check().lookalikeOf}</strong>, a domain you get mail from, but is not the same.
        </div>
      </Show>
      <Show when={!failed() && check().nameSeenFrom}>
        <div class="warning">
          Earlier mail from "{props.view.senderName}" came from <strong>{check().nameSeenFrom}</strong>. This message is from{" "}
          <strong>{props.view.senderAddress}</strong>.
        </div>
      </Show>
      <Show when={!failed() && check().replyElsewhere}>
        <div class="warning">
          Replies to this message would go to <strong>{check().replyTo}</strong>, not to the sender's address.
        </div>
      </Show>
      <Show when={check().firstTime && !check().nameSeenFrom && !check().lookalikeOf}>
        <div class="message-note">This is the first message you have had from {props.view.senderAddress}.</div>
      </Show>
    </div>
  );
}
