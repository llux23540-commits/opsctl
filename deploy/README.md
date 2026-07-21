# 部署 opsctl

opsctl 是**单二进制**服务:Rust 后端在编译期(release)用 rust-embed 把 Vue SPA 内嵌进可执行文件,运行时 `/` 出前端、`/api` 出接口,同源、无需单独的前端服务器。数据默认存 SQLite(单文件)。

## 必须提供的密钥(环境变量)
生产务必覆盖这三项(别用默认值,别写进镜像):
- `OPSCTL_AUTH__JWT_SECRET` — JWT 签名密钥(`openssl rand -hex 32`)
- `OPSCTL_VAULT__PASSPHRASE` — 金库解封口令;设置后启动即自动解封并加密历史明文凭据。**不设 = 金库封存**(带密码的账号建不了、远程执行取不到凭据),需登录后在「设置 → 凭据金库」手动解封。
- `OPSCTL_BOOTSTRAP__ADMIN_PASSWORD` — 首次启动播种的 admin 密码(仅用户表为空时生效)

其它可选:`OPSCTL_SERVER__BIND`(默认 `0.0.0.0:8443`)、`OPSCTL_STORE__URL`、`OPSCTL_DEV__SEED`(生产设 `false`,不植入演示数据)。

## 方式一:docker compose(单机)
```bash
cp deploy/.env.example deploy/.env      # 填入三个密钥
docker compose -f deploy/docker-compose.yml up -d --build
# 打开 http://<host>:8443/  ，用 admin / <你设的密码> 登录
```
数据落在命名卷 `opsctl-data`(容器内 `/app/data`)。

## 方式二:Kubernetes
```bash
docker build -f deploy/Dockerfile -t <registry>/opsctl-server:<tag> .
docker push <registry>/opsctl-server:<tag>
# 用真实值建 Secret(别提交):
kubectl create namespace opsctl
kubectl -n opsctl create secret generic opsctl-secrets \
  --from-literal=jwt-secret="$(openssl rand -hex 32)" \
  --from-literal=vault-passphrase="$(openssl rand -hex 24)" \
  --from-literal=admin-password="$(openssl rand -hex 12)"
# 改 opsctl.yaml 里的 image 与 Ingress host,然后:
kubectl apply -f deploy/k8s/opsctl.yaml
```
说明:SQLite 是单写入,`replicas: 1` + `Recreate` + RWO PVC。要横向扩容需换 PostgreSQL(`OPSCTL_STORE__URL=postgres://…`,后端 store 层已按接口预留,驱动待接)。

## 方式三:裸机单二进制
```bash
cd web && npm ci && npm run build && cd ..
cargo build --release -p opsctl-server         # 此时 SPA 被内嵌进二进制
OPSCTL_AUTH__JWT_SECRET=... OPSCTL_VAULT__PASSPHRASE=... OPSCTL_DEV__SEED=false \
  ./target/release/opsctl-server
```
`target/release/opsctl-server` 可单独拷到目标机运行(自带前端)。生产建议在其前面挂 TLS(反代/Ingress)。

## 升级 / 数据
- 升级:重建镜像 → 滚动(k8s 用 Recreate)。DB schema 在启动 `init()` 幂等迁移。
- 备份:备份 `/app/data/opsctl.db`(以及金库口令——丢了口令=解不开已加密凭据)。

## TLS / HTTPS(边缘加密)

客户端到服务端全程走 HTTPS——登录密码、OTP、SSH/SQL 命令、审批理由不再明文上网。
nginx 边缘终止 TLS,opsctl 应用在 compose 网络内部保持明文(信任边界内),不对宿主暴露。

```sh
cp .env.example .env            # 填 JWT / 金库口令等密钥
sh gen-certs.sh                 # 生成自签证书(内网/自用);生产替换 certs/ 下真证书
docker compose up -d --build    # 起 opsctl(内部)+ nginx(80→443,TLS)
```

- 访问 `https://<host>/`(自签证书浏览器会告警,内网自用属正常)。
- 生产:把 `certs/fullchain.pem` / `certs/privkey.pem` 换成真证书(Let's Encrypt / 内部 CA);
  `nginx.conf` 已指向这两个路径,`server_name _` 可改成真实域名。
- 校验配置:`docker run --rm -v "$PWD/nginx.conf:/etc/nginx/nginx.conf:ro" -v "$PWD/certs:/etc/nginx/certs:ro" nginx:1.27-alpine nginx -t`
- 前端用相对 `/api`,自动跟随 HTTPS,无需改动。
