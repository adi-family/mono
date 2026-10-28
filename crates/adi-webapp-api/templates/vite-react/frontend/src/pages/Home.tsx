// Agent-authored example page: two TanStack Query calls against the backend's example routes,
// rendered with the two shadcn/ui components the scaffold ships. Drop a new file beside this one
// and add it to `src/App.tsx`'s `<Routes>` to add another page.

import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { api } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

/** `/api/notes`, backed by the shared SQLite store via `@adi/db` — see the backend README. */
function Notes() {
  const queryClient = useQueryClient();
  const [draft, setDraft] = useState("");
  const notes = useQuery({
    queryKey: ["notes"],
    queryFn: async () => {
      const res = await api.notes.$get();
      if (!res.ok) throw new Error(`notes: ${res.status}`);
      return (await res.json()).notes;
    },
  });
  const add = useMutation({
    mutationFn: async (body: string) => {
      const res = await api.notes.$post({ json: { body } });
      if (!res.ok) throw new Error(`notes: ${res.status}`);
      return res.json();
    },
    onSuccess: () => {
      setDraft("");
      queryClient.invalidateQueries({ queryKey: ["notes"] });
    },
  });

  return (
    <Card>
      <CardHeader>
        <CardTitle>Notes</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <form
          className="flex gap-2"
          onSubmit={(ev) => {
            ev.preventDefault();
            if (draft.trim()) add.mutate(draft.trim());
          }}
        >
          <input
            className="border-input flex-1 rounded-md border bg-transparent px-3 py-1 text-sm outline-none"
            value={draft}
            onChange={(ev) => setDraft(ev.target.value)}
            placeholder="Write a note"
          />
          <Button type="submit" size="sm" disabled={add.isPending || !draft.trim()}>
            Add
          </Button>
        </form>
        {notes.isPending && <p className="text-muted-foreground text-sm">loading…</p>}
        {notes.data?.length === 0 && (
          <p className="text-muted-foreground text-sm">No notes yet.</p>
        )}
        <ul className="flex flex-col gap-1 text-sm">
          {notes.data?.map((note) => <li key={note.id}>{note.body}</li>)}
        </ul>
      </CardContent>
    </Card>
  );
}

export function Home() {
  const status = useQuery({
    queryKey: ["status"],
    queryFn: async () => {
      const res = await api.status.$get();
      if (!res.ok) throw new Error(`status: ${res.status}`);
      return res.json();
    },
    refetchInterval: 5000,
  });

  return (
    <main className="mx-auto flex max-w-2xl flex-col gap-6 p-8">
      <Card>
        <CardHeader>
          <CardTitle>Backend status</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {status.isPending && <p className="text-muted-foreground text-sm">loading…</p>}
          {status.isError && <p className="text-destructive text-sm">{String(status.error)}</p>}
          {status.data && (
            <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
              {Object.entries(status.data).map(([key, value]) => (
                <div className="contents" key={key}>
                  <dt className="text-muted-foreground">{key}</dt>
                  <dd className="font-mono">{String(value)}</dd>
                </div>
              ))}
            </dl>
          )}
          <Button size="sm" onClick={() => status.refetch()} disabled={status.isFetching}>
            Refresh
          </Button>
        </CardContent>
      </Card>
      <Notes />
    </main>
  );
}
