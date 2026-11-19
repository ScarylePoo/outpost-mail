// Reading a `mailto:` address (RFC 6068) into the start of a message. The
// address comes from outside the app (a link on a web page, say), so only
// the fields that are safe to prefill are taken: recipients, subject and
// text. Anything else it asks for, such as attaching a file, is ignored.

export interface MailtoFields {
  to: string;
  cc: string;
  bcc: string;
  subject: string;
  body: string;
}

function decode(text: string): string {
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
}

/** One line of text: a header's value must not smuggle in another header. */
function oneLine(text: string): string {
  return text.replace(/[\r\n\u0000]+/g, " ").trim();
}

export function parseMailto(link: string): MailtoFields | undefined {
  const match = /^mailto:([^?#]*)(?:\?([^#]*))?/i.exec(link.trim());
  if (!match) return undefined;
  const fields: MailtoFields = { to: oneLine(decode(match[1] ?? "")), cc: "", bcc: "", subject: "", body: "" };
  const add = (existing: string, more: string) => [existing, oneLine(more)].filter(Boolean).join(", ");
  for (const pair of (match[2] ?? "").split("&")) {
    const at = pair.indexOf("=");
    if (at < 0) continue;
    const name = decode(pair.slice(0, at)).toLowerCase();
    const value = decode(pair.slice(at + 1));
    if (name === "to") fields.to = add(fields.to, value);
    else if (name === "cc") fields.cc = add(fields.cc, value);
    else if (name === "bcc") fields.bcc = add(fields.bcc, value);
    else if (name === "subject") fields.subject = oneLine(value);
    else if (name === "body") fields.body = value.replace(/\r\n?/g, "\n").replace(/\u0000/g, "").slice(0, 100_000);
  }
  return fields;
}
