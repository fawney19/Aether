# 快速开始
## 部署

选择适合你的部署方式开始

### 1. 预构建镜像 (Docker Compose)
```markdown
# 1. 克隆代码
git clone https://github.com/fawney19/Aether.git
cd Aether

# 2. 配置环境变量
cp .env.example .env
./generate_keys.sh # 生成密钥, 并将生成的密钥填入 .env

# 3. 部署 / 更新（自动执行数据库迁移）
docker compose pull && docker compose up -d

# 4. 升级前备份
docker compose exec postgres pg_dump -U postgres aether | gzip > backup_$(date +%Y%m%d_%H%M%S).sql.gz
```

### 2. 本地代码构建镜像 (Docker Compose)
```markdown
# 1. 克隆代码
git clone https://github.com/fawney19/Aether.git
cd Aether

# 2. 配置环境变量
cp .env.example .env
./generate_keys.sh  # 生成密钥, 并将生成的密钥填入 .env

# 3. 构建（自动构建、启动、迁移）
./deploy.sh

# 4. 更新需要拉取最新代码
git pull origin main
```

### 3. 本地开发
依赖 Docker、uv、nodejs
```markdown
# 启动数据库
docker compose -f docker-compose.build.yml up -d postgres redis

# 后端
uv sync
./dev.sh

# 前端
cd frontend && npm install && npm run dev
```

## 配置流程

1. **创建统一模型**
   以Opus4.6为例, 其他模型同样添加即可, 非必要建议只添加官方支持的模型ID
   ![image.png](/Aether%E4%BD%BF%E7%94%A8%E6%95%99%E7%A8%8B/image.png)

2. **添加提供商**
   ![image.png](/Aether%E4%BD%BF%E7%94%A8%E6%95%99%E7%A8%8B/image%201.png)

3. **添加端点**
   ![image.png](/Aether%E4%BD%BF%E7%94%A8%E6%95%99%E7%A8%8B/image%202.png)
   ![image.png](/Aether%E4%BD%BF%E7%94%A8%E6%95%99%E7%A8%8B/image%203.png)

4. **添加密钥**
   ![image.png](/Aether%E4%BD%BF%E7%94%A8%E6%95%99%E7%A8%8B/image%204.png)

5. **关联全局模型**
   ![image.png](/Aether%E4%BD%BF%E7%94%A8%E6%95%99%E7%A8%8B/image%205.png)
   ![image.png](/Aether%E4%BD%BF%E7%94%A8%E6%95%99%E7%A8%8B/image%206.png)

6. **模型映射**
   ![image.png](/Aether%E4%BD%BF%E7%94%A8%E6%95%99%E7%A8%8B/image%207.png)

## 反向代理

添加提供商时, 提供商类型选择对应类型即可, 反向代理默认开启提供商级格式转换。

1. **Codex**
   - OAuth授权登录
   - 导入RefreshToken, 支持批量导入
2. **Kiro**
   - Build ID
   - Identity Center
     - Start URL
     - Region
   - 导入 RefreshToken, 支持批量导入
     - Social 格式要求
       ```json
       {
           "refresh_token": ""
       }
       ```
     - IDC 格式要求
       ```json
       {
           "refresh_token": "",
           "client_id": "",
           "client_secret": "",
           "machine_id": ""
       }
       ```
3. **Antigravity**
   - OAuth授权登录
   - 导入RefreshToken, 支持批量导入

![image.png](/Aether%E4%BD%BF%E7%94%A8%E6%95%99%E7%A8%8B/image%208.png)

## 异步任务

需要有提供商端点支持

1. Veo
2. Sora

## 代理配置

1. **Aether-Proxy**
   Rust实现, 超小资源占有, 适合性能低的vps直接使用。
   [https://github.com/fawney19/Aether/tree/main/aether-proxy](https://github.com/fawney19/Aether/tree/main/aether-proxy)

2. **代理节点**
   在模块管理中, 开启代理模块后可以添加和使用代理功能, 包括手动添加和Aether-Proxy自动连接。

3. **多级代理**
   优先级: Key代理 > 提供商代理 > 全局代理
   - 全局代理 - 系统配置
   - 提供商代理 - 提供商配置
   - Key代理 - Key配置
