// What the calendar, tasks and people views share: the accounts and their
// calendars, and a counter that says "something changed, look again".

import { createSignal } from "solid-js";
import { core, type DavAccountInfo, type DavCollectionInfo } from "../core";

const [davAccounts, setDavAccounts] = createSignal<DavAccountInfo[]>([]);
const [davVersion, setDavVersion] = createSignal(0);
export { davAccounts, davVersion };

export function loadDavAccounts(): Promise<void> {
  return core
    .send({ type: "listDavAccounts" })
    .then((r) => {
      setDavAccounts(r.accounts);
    })
    .catch(() => {});
}

/** Called when the core says calendars, tasks or contacts changed. */
export function davChanged() {
  void loadDavAccounts();
  setDavVersion((v) => v + 1);
}

/** A collection the user can put new things of one kind in. */
export interface Place {
  account: string;
  collection: DavCollectionInfo;
  label: string;
}

export function places(kind: "events" | "tasks" | "contacts"): Place[] {
  const many = davAccounts().length > 1;
  return davAccounts().flatMap((account) =>
    account.collections
      .filter((c) => c.enabled && c[kind])
      .map((collection) => ({ account: account.id, collection, label: many ? `${collection.name} (${account.username})` : collection.name })),
  );
}

export const placeKey = (place: Place) => `${place.account}#${place.collection.id}`;
export const findPlace = (all: Place[], key: string) => all.find((p) => placeKey(p) === key);
