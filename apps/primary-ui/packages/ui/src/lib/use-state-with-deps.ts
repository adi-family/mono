import * as React from "react";

type Snapshot<T> = { deps: React.DependencyList; value: T };

function sameDeps(previous: React.DependencyList, next: React.DependencyList) {
  return previous.length === next.length && previous.every((item, index) => Object.is(item, next[index]));
}

/**
 * `useState` whose value is read again from `read()` whenever `deps` change — `useMemo` with a
 * setter. Use it for local edits of something that comes from outside (browser storage, the
 * record a prop selects): the user changes the value freely, and switching the source starts
 * over from what that source holds. The reset happens during render, so no frame shows the
 * previous source's value and no effect has to trail behind the dependency list.
 */
export function useStateWithDeps<T>(
  read: () => T,
  deps: React.DependencyList,
): [T, React.Dispatch<React.SetStateAction<T>>] {
  const [snapshot, setSnapshot] = React.useState<Snapshot<T>>(() => ({ deps, value: read() }));

  let current = snapshot;
  if (!sameDeps(snapshot.deps, deps)) {
    current = { deps, value: read() };
    setSnapshot(current);
  }

  const setValue = React.useCallback<React.Dispatch<React.SetStateAction<T>>>((update) => {
    setSnapshot((previous) => ({
      deps: previous.deps,
      value: typeof update === "function" ? (update as (previous: T) => T)(previous.value) : update,
    }));
  }, []);

  return [current.value, setValue];
}
