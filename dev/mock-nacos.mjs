// 本地验证用的 mock Nacos:按 Nacos v1/v2 Open API 的文档响应形状实现,
// 覆盖 opsctl 实际调用的全部端点。没有 docker / 真实 Nacos 时用它点通整条链路。
//
// 端点分组:
//   鉴权        POST   /v1/auth/login
//   探活 / 集群  GET    /v1/console/health/readiness · /v2/core/cluster/nodes
//   配置        GET    /v1/cs/configs(读 / search 列表) · POST 发布
//   命名空间     GET    /v2/console/namespace{,/list}
//              CRUD   /v1/console/namespaces(含 checkNamespaceIdExist 重名探测)
//   用户        CRUD   /v1/auth/users
//   角色        GET / POST / DELETE /v1/auth/roles
//   权限        GET / POST / DELETE /v1/auth/permissions
//
// 这些接口的响应形状在真实 Nacos 里就是不对称的,别"顺手统一"——opsctl 的解析分支
// 正是照着这份不对称写的,mock 抹平了测试就会说谎:
//   用户 / 角色 / 权限 列表 -> 裸 Page<T> {totalCount,pageNumber,pagesAvailable,pageItems},无外壳
//   用户 / 角色 / 权限 写入 -> RestResult {code:200,message:null,data:"xxx ok!"}
//   命名空间 列表          -> RestResult {code:200,data:[…]}
//   命名空间 增 / 改 / 删    -> 裸 boolean,失败也照样 HTTP 200
//   参数非法               -> HTTP 400 + 纯文本(Spring 兜 IllegalArgumentException,不是 JSON)
//   /v3/**                -> 404,好让 opsctl 的 flavor 探测落回 v1
//
//   node dev/mock-nacos.mjs            # 监听 127.0.0.1:18848
//   PORT=8848 node dev/mock-nacos.mjs  # 换端口
//
// 账号:nacos / nacos-pass(留空用户名则 opsctl 走免鉴权分支,本 mock 仍会拒绝——
// 想测免鉴权把 REQUIRE_AUTH 改成 false)。
import http from 'node:http';

const PORT = Number(process.env.PORT || 18848);
const REQUIRE_AUTH = process.env.REQUIRE_AUTH !== 'false';
const USER = process.env.NACOS_USER || 'nacos';
const PASS = process.env.NACOS_PASS || 'nacos-pass';

/** `${tenant}|${group}|${dataId}` -> { content, type } */
const configs = new Map();

/** namespaceId -> namespace object(public 恒存在,type=0) */
const namespaces = new Map([
  ['', { namespace: '', namespaceShowName: 'public', namespaceDesc: null, quota: 200, configCount: 0, type: 0 }],
]);

/** username -> { username, password };password 存 bcrypt 摘要,真实 Nacos 的列表接口就是这么把它漏出来的 */
const users = new Map([
  ['nacos', { username: 'nacos', password: '$2a$10$EuWPZHzz32dJN7jexM34MOeYirDdFAZm2kuWj7VEkUwUxjBpFDBLu' }],
]);

/** [{ role, username }];ROLE_ADMIN 由 Nacos 初始化时写死,接口层不允许再创建 */
const roles = [{ role: 'ROLE_ADMIN', username: 'nacos' }];

/** [{ role, resource, action }];resource = `<namespaceId>:<group>:<type>/<name>` */
const permissions = [];

// 1.x / 2.x 的鉴权列表接口直接吐 Page<T> 裸对象,没有 RestResult 外壳。
const page = (items, q) => {
  const pageNo = Math.max(1, Number(q.get('pageNo')) || 1);
  const pageSize = Math.max(1, Number(q.get('pageSize')) || 10);
  const from = (pageNo - 1) * pageSize;
  return {
    totalCount: items.length,
    pageNumber: pageNo,
    pagesAvailable: Math.max(1, Math.ceil(items.length / pageSize)),
    pageItems: items.slice(from, from + pageSize),
  };
};

// 鉴权写接口统一 RestResult,message 恒为 null,data 是一句人话。
const rest = (data) => ({ code: 200, message: null, data });

// customNamespaceId 留空时由服务端发号,真实实现用 UUID.randomUUID()。
const uuidish = () =>
  'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    return (c === 'x' ? r : (r & 0x3) | 0x8).toString(16);
  });

const readBody = (req) =>
  new Promise((r) => {
    let b = '';
    req.on('data', (c) => (b += c));
    req.on('end', () => r(b));
  });

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://${req.headers.host}`);
  const p = url.pathname;
  const q = url.searchParams;
  const send = (code, body, type = 'application/json') => {
    res.writeHead(code, { 'content-type': type });
    res.end(typeof body === 'string' ? body : JSON.stringify(body));
  };
  const authed = () => !REQUIRE_AUTH || q.get('accessToken') === 'mock-token';

  // 3.x 的 /v3/** 在这台 mock 上不存在:opsctl 探到 404 就把 flavor 落回 v1。
  if (p.startsWith('/nacos/v3/')) return send(404, 'not found', 'text/plain');

  // 鉴权
  if (p === '/nacos/v1/auth/login' && req.method === 'POST') {
    const form = new URLSearchParams(await readBody(req));
    if (form.get('username') === USER && form.get('password') === PASS) {
      return send(200, { accessToken: 'mock-token', tokenTtl: 18000, globalAdmin: true });
    }
    return send(403, 'unknown user!', 'text/plain');
  }

  // 存活探测(opsctl 的降级路径)
  if (p === '/nacos/v1/console/health/readiness') return send(200, 'ok', 'text/plain');

  // 集群成员
  if (p === '/nacos/v2/core/cluster/nodes') {
    if (!authed()) return send(403, 'no token', 'text/plain');
    return send(200, {
      code: 0,
      message: 'success',
      data: [
        { ip: '10.12.0.11', port: 8848, state: 'UP', address: '10.12.0.11:8848', extendInfo: { version: '2.3.2' } },
        { ip: '10.12.0.12', port: 8848, state: 'UP', address: '10.12.0.12:8848', extendInfo: { version: '2.3.2' } },
        { ip: '10.12.0.13', port: 8848, state: 'DOWN', address: '10.12.0.13:8848', extendInfo: { version: '2.3.2' } },
      ],
    });
  }

  // 命名空间(v2 console API,形状按官方文档 /docs/v2/guide/user/open-api 第 3 章)
  if (p === '/nacos/v2/console/namespace/list') {
    if (!authed()) return send(403, 'no token', 'text/plain');
    return send(200, { code: 0, message: 'success', data: [...namespaces.values()] });
  }
  if (p === '/nacos/v2/console/namespace') {
    if (!authed()) return send(403, 'no token', 'text/plain');
    if (req.method === 'GET') {
      const ns = namespaces.get(q.get('namespaceId') || '');
      return ns
        ? send(200, { code: 0, message: 'success', data: ns })
        : send(200, { code: 22001, message: 'namespace not exist', data: null });
    }
    if (req.method === 'POST' || req.method === 'PUT') {
      const f = new URLSearchParams(await readBody(req));
      const id = f.get('namespaceId') || '';
      const exists = namespaces.has(id);
      if (req.method === 'POST' && exists) {
        return send(200, { code: 22002, message: 'namespace already exist', data: false });
      }
      if (req.method === 'PUT' && !exists) {
        return send(200, { code: 22001, message: 'namespace not exist', data: false });
      }
      namespaces.set(id, {
        namespace: id,
        namespaceShowName: f.get('namespaceName') || id,
        namespaceDesc: f.get('namespaceDesc') || null,
        quota: 200,
        configCount: 0,
        type: 2,
      });
      console.log(`[namespace ${req.method === 'POST' ? 'create' : 'update'}] ${id}`);
      return send(200, { code: 0, message: 'success', data: true });
    }
    if (req.method === 'DELETE') {
      const id = q.get('namespaceId') || '';
      const had = namespaces.delete(id);
      console.log(`[namespace delete] ${id} -> ${had}`);
      return send(200, { code: 0, message: 'success', data: had });
    }
  }

  // ── 用户 / 角色 / 权限(v1 flavor)──────────────────────────────────────────
  // 2.x 上这三个列表接口带 Spring 的 `params = "search=accurate"` 断言:少了 search
  // 会被路由层挡在 controller 之外,报文是纯文本 400,所以这里也得先挡一道。
  // 返回 true 表示 400 已经写回去了,调用方直接 return。
  const noSearch = () => {
    if (q.has('search')) return false;
    send(400, 'Parameter conditions "search=accurate" not met', 'text/plain');
    return true;
  };
  // search=accurate 精确匹配,search=blur 子串匹配;关键字为空一律放行。
  const kwMatch = (val, kw) => (!kw ? true : q.get('search') === 'blur' ? String(val).includes(kw) : val === kw);

  if (p === '/nacos/v1/auth/users') {
    if (!authed()) return send(403, 'no token', 'text/plain');
    if (req.method === 'GET') {
      if (noSearch()) return;
      const items = [...users.values()]
        .filter((u) => kwMatch(u.username, q.get('username') || ''))
        // bcrypt 摘要照吐不误:opsctl 必须自己剥掉,mock 不吐就测不出这个洞。
        .map((u) => ({ username: u.username, password: u.password }));
      return send(200, page(items, q));
    }
    if (req.method === 'POST') {
      const f = new URLSearchParams(await readBody(req));
      const username = f.get('username') || '';
      if (users.has(username)) return send(400, `user '${username}' already exist!`, 'text/plain');
      users.set(username, { username, password: f.get('password') || '' });
      console.log(`[user create] ${username}`);
      return send(200, rest('create user ok!'));
    }
    if (req.method === 'PUT') {
      const f = new URLSearchParams(await readBody(req));
      const username = f.get('username') || '';
      const u = users.get(username);
      if (!u) return send(400, `user '${username}' not exist!`, 'text/plain');
      u.password = f.get('newPassword') || '';
      console.log(`[user update] ${username}`);
      return send(200, rest('update user ok!'));
    }
    if (req.method === 'DELETE') {
      const username = q.get('username') || '';
      // 真实 Nacos 不许删掉持有 ROLE_ADMIN 的账号,复现同一个坑。
      if (roles.some((r) => r.username === username && r.role === 'ROLE_ADMIN')) {
        return send(400, `cannot delete admin: ${username}`, 'text/plain');
      }
      users.delete(username);
      console.log(`[user delete] ${username}`);
      return send(200, rest('delete user ok!'));
    }
  }

  if (p === '/nacos/v1/auth/roles') {
    if (!authed()) return send(403, 'no token', 'text/plain');
    if (req.method === 'GET') {
      if (noSearch()) return;
      const items = roles
        .filter((r) => kwMatch(r.username, q.get('username') || '') && kwMatch(r.role, q.get('role') || ''))
        .map((r) => ({ role: r.role, username: r.username }));
      return send(200, page(items, q));
    }
    if (req.method === 'POST') {
      const f = new URLSearchParams(await readBody(req));
      const role = f.get('role') || '';
      const username = f.get('username') || '';
      // ROLE_ADMIN 只能由 Nacos 自己在初始化时写入。
      if (role === 'ROLE_ADMIN') {
        return send(400, "role 'ROLE_ADMIN' is not permitted to create!", 'text/plain');
      }
      roles.push({ role, username });
      console.log(`[role bind] ${role}->${username}`);
      return send(200, rest('add role ok!'));
    }
    if (req.method === 'DELETE') {
      const role = q.get('role') || '';
      const username = q.get('username') || '';
      // username 留空 = 把这个角色从所有用户身上摘掉(官方 controller 的分支)。
      for (let i = roles.length - 1; i >= 0; i -= 1) {
        if (roles[i].role === role && (!username || roles[i].username === username)) roles.splice(i, 1);
      }
      console.log(`[role unbind] ${role}->${username || '*'}`);
      return send(200, rest(`delete role of user ${username || 'all'} ok!`));
    }
  }

  if (p === '/nacos/v1/auth/permissions') {
    if (!authed()) return send(403, 'no token', 'text/plain');
    if (req.method === 'GET') {
      if (noSearch()) return;
      const items = permissions
        .filter((x) => kwMatch(x.role, q.get('role') || ''))
        .map((x) => ({ role: x.role, resource: x.resource, action: x.action }));
      return send(200, page(items, q));
    }
    if (req.method === 'POST') {
      const f = new URLSearchParams(await readBody(req));
      const role = f.get('role') || '';
      const resource = f.get('resource') || '';
      const action = f.get('action') || '';
      // 赋权前角色必须已经存在(真集群还有 ~15s 传播延迟),顺序反了就是这条错。
      if (!roles.some((r) => r.role === role)) return send(400, `role ${role} not found!`, 'text/plain');
      permissions.push({ role, resource, action });
      console.log(`[perm grant] ${role} ${resource} ${action}`);
      return send(200, rest('add permission ok!'));
    }
    if (req.method === 'DELETE') {
      const role = q.get('role') || '';
      const resource = q.get('resource') || '';
      const action = q.get('action') || '';
      const i = permissions.findIndex((x) => x.role === role && x.resource === resource && x.action === action);
      if (i >= 0) permissions.splice(i, 1);
      console.log(`[perm revoke] ${role} ${resource} ${action}`);
      return send(200, rest('delete permission ok!'));
    }
  }

  // 命名空间(v1 console API):和上面的 v2 handler 共用同一份 namespaces,别分叉。
  // 列表是 RestResult,增删改却是裸 boolean——这处落差就是 opsctl 要区分 flavor 的原因。
  if (p === '/nacos/v1/console/namespaces') {
    if (!authed()) return send(403, 'no token', 'text/plain');
    if (req.method === 'GET') {
      // 重名探测和列表共用路径,先看 query 再决定走哪支。
      if (q.get('checkNamespaceIdExist') === 'true') {
        const cid = q.get('customNamespaceId') || '';
        return send(200, cid !== '' && namespaces.has(cid));
      }
      return send(200, { code: 200, message: null, data: [...namespaces.values()] });
    }
    if (req.method === 'POST') {
      const f = new URLSearchParams(await readBody(req));
      let id = f.get('customNamespaceId') || '';
      const name = f.get('namespaceName') || '';
      if (!id) id = uuidish();
      // id 只收 ^[\w-]+ 且 ≤128 字符;名字不能带 @#$%^&* —— 校验不过返回裸 false,HTTP 仍是 200。
      else if (!/^[\w-]+$/.test(id) || id.length > 128 || namespaces.has(id)) return send(200, false);
      if (!name || /[@#$%^&*]/.test(name)) return send(200, false);
      namespaces.set(id, {
        namespace: id,
        namespaceShowName: name,
        namespaceDesc: f.get('namespaceDesc') || null,
        quota: 200,
        configCount: 0,
        type: 2,
      });
      console.log(`[namespace create] ${id}`);
      return send(200, true);
    }
    if (req.method === 'PUT') {
      const f = new URLSearchParams(await readBody(req));
      // 改用的是 namespace / namespaceShowName,和创建时的字段名不一样,别抄串。
      const ns = namespaces.get(f.get('namespace') || '');
      if (!ns) return send(200, false);
      ns.namespaceShowName = f.get('namespaceShowName') || ns.namespaceShowName;
      ns.namespaceDesc = f.get('namespaceDesc') || null;
      console.log(`[namespace update] ${ns.namespace}`);
      return send(200, true);
    }
    if (req.method === 'DELETE') {
      const id = q.get('namespaceId') || '';
      // public 不是库里的行,真实 Nacos 也删不掉。
      if (id === '') return send(200, false);
      const had = namespaces.delete(id);
      console.log(`[namespace delete] ${id} -> ${had}`);
      return send(200, had);
    }
  }

  // 配置读 / 列表 / 发布(同一路径,靠 search 参数区分)
  if (p === '/nacos/v1/cs/configs') {
    if (!authed()) return send(403, 'no token', 'text/plain');
    const tenant = q.get('tenant') || '';
    if (req.method === 'GET' && q.has('search')) {
      const items = [...configs.entries()]
        .filter(([k]) => k.startsWith(`${tenant}|`))
        .map(([k, v]) => {
          const [, group, dataId] = k.split('|');
          return { id: '1', dataId, group, content: v.content, type: v.type, appName: '' };
        });
      return send(200, { totalCount: items.length, pageNumber: 1, pagesAvailable: 1, pageItems: items });
    }
    if (req.method === 'GET') {
      const hit = configs.get(`${tenant}|${q.get('group')}|${q.get('dataId')}`);
      return hit ? send(200, hit.content, 'text/plain') : send(404, 'config data not exist', 'text/plain');
    }
    if (req.method === 'POST') {
      const f = new URLSearchParams(await readBody(req));
      configs.set(`${f.get('tenant') || ''}|${f.get('group')}|${f.get('dataId')}`, {
        content: f.get('content') || '',
        type: f.get('type') || 'text',
      });
      console.log(`[publish] ${f.get('group')}/${f.get('dataId')}  (${(f.get('content') || '').length} 字节)`);
      return send(200, 'true', 'text/plain');
    }
  }

  send(404, 'not found', 'text/plain');
});

server.listen(PORT, '127.0.0.1', () =>
  console.log(`mock nacos on http://127.0.0.1:${PORT}/nacos  (user=${USER} pass=${PASS} auth=${REQUIRE_AUTH})`)
);
