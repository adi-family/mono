// Agent-authored example page: one TanStack Query call against the backend's example route,
// rendered with the two shadcn/ui components the scaffold ships. Drop a new file beside this one
// and add it to `src/App.tsx`'s `<Routes>` to add another page.

import { useQuery } from "@tanstack/react-query";

import { api } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

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
    <main className="mx-auto max-w-2xl p-8">
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
    </main>
  );
}
