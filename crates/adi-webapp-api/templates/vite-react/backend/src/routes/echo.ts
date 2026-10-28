// Example route: POST /api/echo — demonstrates request validation with @hono/zod-validator.
// A body that fails `schema` never reaches the handler: the validator answers its own 400, and
// `c.req.valid("json")` below is already the parsed, typed body.

import { zValidator } from "@hono/zod-validator";
import { Hono } from "hono";
import { z } from "zod";

const schema = z.object({ message: z.string().min(1) });

export const echoRoute = new Hono().post("/", zValidator("json", schema), (c) => {
  const { message } = c.req.valid("json");
  return c.json({ echoed: message, length: message.length });
});
