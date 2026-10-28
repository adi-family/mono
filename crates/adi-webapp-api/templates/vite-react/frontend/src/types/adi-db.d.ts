// A stub for `@adi/db`'s public shape, remapped in here by `tsconfig.app.json`'s `paths`. The
// real module lives in the mono store's own `node_modules` (seeded by the platform, well outside
// this scaffold — see `guides/db.md`) and its implementation imports `bun:sqlite` and Node's
// `process`/`fs`/`os`/`path`: fine for the backend's own typecheck (it has `@types/bun`), but
// pulled into *this* program only for `AppType`'s structural type, which has no business knowing
// any of that. Without this stub, typechecking the frontend would also typecheck `@adi/db`'s
// whole implementation and fail on every ambient global it uses that this program doesn't
// declare. The same trick applies to any other backend dependency chained into `app.ts` whose own
// types reach for a Node/Bun global — route files that stay to Hono, zod and Web-platform globals
// never need one.
declare module "@adi/db" {
  export function query<T = Record<string, unknown>>(sql: string, ...params: unknown[]): T[];
  export function get<T = Record<string, unknown>>(sql: string, ...params: unknown[]): T | null;
  export function run(
    sql: string,
    ...params: unknown[]
  ): { changes: number; lastInsertRowid: number | bigint };
  export function exec(sql: string): void;
}
