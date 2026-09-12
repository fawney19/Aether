FROM node:22-alpine

ENV NODE_ENV=production \
    HOST=0.0.0.0 \
    PORT=8788 \
    AETHER_VSCODEX_DATA_DIR=/var/lib/aether-vscodex

WORKDIR /app

COPY package.json package-lock.json ./
RUN npm ci --omit=dev && npm cache clean --force

COPY cloud ./cloud
COPY relay ./relay
COPY public ./public

RUN mkdir -p /var/lib/aether-vscodex && chown -R node:node /var/lib/aether-vscodex /app

USER node

EXPOSE 8788

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD node -e "fetch('http://127.0.0.1:8788/healthz').then(r=>{if(!r.ok)process.exit(1)}).catch(()=>process.exit(1))"

CMD ["node", "cloud/server.js"]
