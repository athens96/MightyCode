import { createContext, useContext, useRef, useSyncExternalStore } from 'react';
import type { Bridge } from '../bridge/bridge';
import type { Translate } from '../i18n/locale';
import type { Store, StoreState } from '../store/store';

export interface AppServices {
  bridge: Bridge;
  store: Store;
  t: Translate;
}

export const ServicesContext = createContext<AppServices | null>(null);

export function useServices(): AppServices {
  const services = useContext(ServicesContext);
  if (!services) throw new Error('ServicesContext is missing');
  return services;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  if (typeof value !== 'object' || value === null) return false;
  const proto = Object.getPrototypeOf(value);
  return proto === Object.prototype || proto === null;
}

/** Shallow equality for primitives, arrays/tuples and plain objects (by Object.is per element/key). */
export function shallowEqual<T>(a: T, b: T): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== 'object' || a === null || typeof b !== 'object' || b === null) return false;
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i += 1) {
      if (!Object.is(a[i], b[i])) return false;
    }
    return true;
  }
  if (!isPlainObject(a) || !isPlainObject(b)) return false;
  const keysA = Object.keys(a);
  const keysB = Object.keys(b);
  if (keysA.length !== keysB.length) return false;
  for (const key of keysA) {
    if (!Object.prototype.hasOwnProperty.call(b, key) || !Object.is(a[key], b[key])) return false;
  }
  return true;
}

/**
 * Builds a referentially stable `getSnapshot` function for `useSyncExternalStore` that caches the last selected value
 * and returns the previous reference whenever `equals(prev, next)` holds.
 */
export function createSelectorSnapshot<T>(
  getState: () => StoreState,
  getSelector: () => (state: StoreState) => T,
  getEquals: () => (a: T, b: T) => boolean,
): () => T {
  let hasValue = false;
  let lastState: StoreState | undefined;
  let lastSelector: ((state: StoreState) => T) | undefined;
  let lastValue = undefined as T;

  return () => {
    const state = getState();
    const selector = getSelector();
    if (hasValue && lastState === state && lastSelector === selector) {
      return lastValue;
    }
    const next = selector(state);
    if (hasValue && getEquals()(lastValue, next)) {
      lastState = state;
      lastSelector = selector;
      return lastValue;
    }
    hasValue = true;
    lastState = state;
    lastSelector = selector;
    lastValue = next;
    return next;
  };
}

/** Selects a slice of the store state and re-renders only when `equals(previous, next)` is false. */
export function useStoreSelector<T>(
  selector: (state: StoreState) => T,
  equals: (a: T, b: T) => boolean = Object.is,
): T {
  const { store } = useServices();
  const latest = useRef({ selector, equals });
  latest.current = { selector, equals };
  const snapshotRef = useRef<{ store: Store; get: () => T } | null>(null);
  if (!snapshotRef.current || snapshotRef.current.store !== store) {
    snapshotRef.current = {
      store,
      get: createSelectorSnapshot(
        store.getState,
        () => latest.current.selector,
        () => latest.current.equals,
      ),
    };
  }
  const getSnapshot = snapshotRef.current.get;
  return useSyncExternalStore(store.subscribe, getSnapshot, getSnapshot);
}

/** The store's whole state; a component re-renders when it is replaced. */
export function useStoreState(): StoreState {
  const { store } = useServices();
  return useSyncExternalStore(store.subscribe, store.getState, store.getState);
}
