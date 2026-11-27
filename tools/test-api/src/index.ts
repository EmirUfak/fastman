import crypto from "node:crypto";
import cookieParser from "cookie-parser";
import express, { type NextFunction, type Request, type Response } from "express";

type Item = {
  id: string;
  slug: string;
  name: string;
  status: string;
  version: number;
  created_at: string;
};

type Session = {
  sid: string;
  token: string;
  username: string;
};

const PORT = Number(process.env.PORT ?? 4010);
const DEMO_USER = process.env.TEST_USER ?? "demo";
const DEMO_PASS = process.env.TEST_PASS ?? "secret";

const sessionsBySid = new Map<string, Session>();
const sessionsByToken = new Map<string, Session>();
const items = new Map<string, Item>();

const app = express();
app.use(express.json());
app.use(cookieParser());

function resolveAuth(req: Request): {
  session: Session | null;
  hasCookie: boolean;
  hasBearer: boolean;
} {
  const sid = typeof req.cookies?.fastman_sid === "string" ? req.cookies.fastman_sid : null;
  const cookieSession = sid ? (sessionsBySid.get(sid) ?? null) : null;

  const authHeader = req.header("authorization") ?? "";
  const bearerToken = authHeader.toLowerCase().startsWith("bearer ")
    ? authHeader.slice(7).trim()
    : null;
  const bearerSession = bearerToken ? (sessionsByToken.get(bearerToken) ?? null) : null;

  return {
    session: bearerSession ?? cookieSession,
    hasCookie: cookieSession !== null,
    hasBearer: bearerSession !== null,
  };
}

function requireAuth(req: Request, res: Response, next: NextFunction) {
  const { session } = resolveAuth(req);
  if (!session) {
    res.status(401).json({ error: "unauthorized", message: "Valid cookie or Bearer token required" });
    return;
  }
  next();
}

app.get("/health", (_req, res) => {
  res.json({
    ok: true,
    items: items.size,
    sessions: sessionsBySid.size,
  });
});

app.post("/auth/login", (req, res) => {
  const username = typeof req.body?.username === "string" ? req.body.username : DEMO_USER;
  const password = typeof req.body?.password === "string" ? req.body.password : DEMO_PASS;
  if (username !== DEMO_USER || password !== DEMO_PASS) {
    res.status(401).json({ error: "invalid_credentials" });
    return;
  }

  const sid = crypto.randomBytes(16).toString("hex");
  const token = `fm_${crypto.randomBytes(18).toString("hex")}`;
  const session: Session = { sid, token, username };
  sessionsBySid.set(sid, session);
  sessionsByToken.set(token, session);

  res.cookie("fastman_sid", sid, {
    httpOnly: true,
    sameSite: "lax",
    path: "/",
  });
  res.setHeader("X-Auth-Token", token);
  res.status(200).json({
    user: { id: "u_1", username },
    auth: {
      access_token: token,
      token_type: "Bearer",
    },
  });
});

app.get("/auth/me", (req, res) => {
  const { session, hasCookie, hasBearer } = resolveAuth(req);
  if (!session) {
    res.status(401).json({ error: "unauthorized", has_cookie: false, has_bearer: false });
    return;
  }
  const authVia =
    hasCookie && hasBearer ? "cookie+bearer" : hasBearer ? "bearer" : "cookie";
  res.json({
    user: { id: "u_1", username: session.username },
    auth_via: authVia,
    has_cookie: hasCookie,
    has_bearer: hasBearer,
  });
});

app.get("/auth/cookie-only", (req, res) => {
  const { session, hasCookie } = resolveAuth(req);
  if (!session || !hasCookie) {
    res.status(401).json({ error: "cookie_required", has_cookie: false });
    return;
  }
  res.json({ ok: true, user: session.username, has_cookie: true });
});

app.get("/auth/basic", (req, res) => {
  const header = req.header("authorization") ?? "";
  if (!header.toLowerCase().startsWith("basic ")) {
    res.status(401).json({ error: "basic_auth_required" });
    return;
  }
  const decoded = Buffer.from(header.slice(6).trim(), "base64").toString("utf8");
  const [user, pass] = decoded.split(":", 2);
  if (user !== DEMO_USER || pass !== DEMO_PASS) {
    res.status(401).json({ error: "invalid_basic_credentials" });
    return;
  }
  res.json({ ok: true, user });
});

app.post("/auth/logout", (req, res) => {
  const { session } = resolveAuth(req);
  if (session) {
    sessionsBySid.delete(session.sid);
    sessionsByToken.delete(session.token);
  }
  res.clearCookie("fastman_sid", { path: "/" });
  res.status(204).end();
});

app.post("/items", requireAuth, (req, res) => {
  const rawName = typeof req.body?.name === "string" && req.body.name.trim()
    ? req.body.name.trim()
    : `item-${Date.now()}`;
  const status = typeof req.body?.status === "string" ? req.body.status : "active";
  const id = crypto.randomUUID();
  const slug = `${rawName.toLowerCase().replace(/[^a-z0-9]+/g, "-")}-${id.slice(0, 6)}`;
  const item: Item = {
    id,
    slug,
    name: rawName,
    status,
    version: 1,
    created_at: new Date().toISOString(),
  };
  items.set(id, item);
  res.status(201).json(item);
});

app.get("/items", requireAuth, (_req, res) => {
  const list = Array.from(items.values());
  res.json({ items: list, total: list.length });
});

app.get("/items/:id", requireAuth, (req, res) => {
  const item = items.get(req.params.id);
  if (!item) {
    res.status(404).json({ error: "not_found" });
    return;
  }
  res.json(item);
});

app.patch("/items/:id", requireAuth, (req, res) => {
  const item = items.get(req.params.id);
  if (!item) {
    res.status(404).json({ error: "not_found" });
    return;
  }
  if (typeof req.body?.name === "string" && req.body.name.trim()) {
    item.name = req.body.name.trim();
  }
  if (typeof req.body?.status === "string" && req.body.status.trim()) {
    item.status = req.body.status.trim();
  }
  item.version += 1;
  res.json(item);
});

app.delete("/items/:id", requireAuth, (req, res) => {
  if (!items.delete(req.params.id)) {
    res.status(404).json({ error: "not_found" });
    return;
  }
  res.status(204).end();
});

app.get("/delay/:ms", async (req, res) => {
  const ms = Math.min(Math.max(Number(req.params.ms) || 0, 0), 5000);
  await Bun.sleep(ms);
  res.json({ delayed_ms: ms });
});

app.get("/status/:code", (req, res) => {
  const code = Number(req.params.code);
  const status = Number.isInteger(code) && code >= 100 && code <= 599 ? code : 400;
  res.status(status).json({ status });
});

app.post("/reset", (_req, res) => {
  sessionsBySid.clear();
  sessionsByToken.clear();
  items.clear();
  res.status(204).end();
});

app.listen(PORT, "0.0.0.0", () => {
  console.log(`fastman-test-api listening on http://127.0.0.1:${PORT}`);
});
