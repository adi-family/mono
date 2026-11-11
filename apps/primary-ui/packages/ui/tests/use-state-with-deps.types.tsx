import { useStateWithDeps } from "../src/components";

export function acceptedStateWithDepsExamples(storageKey: string | undefined, record: { id: string; name: string } | null) {
  const [width, setWidth] = useStateWithDeps<number | undefined>(() => undefined, [storageKey]);
  setWidth(240);
  setWidth(undefined);
  setWidth((previous) => (previous ?? 200) + 10);

  const [name, setName] = useStateWithDeps(() => record?.name ?? "", [record?.id]);
  setName(name.trim());
  setName((previous) => `${previous}!`);

  const widthValue: number | undefined = width;
  const nameValue: string = name;
  return { widthValue, nameValue };
}

export function rejectedStateWithDepsExamples() {
  const [, setWidth] = useStateWithDeps(() => 200, []);
  // @ts-expect-error — the setter only accepts the reader's type.
  setWidth("wide");
  // @ts-expect-error — the dependency list is required, as with useMemo.
  useStateWithDeps(() => 200);
}
