FROM node:20-slim

RUN corepack enable && corepack prepare pnpm@latest --activate

WORKDIR /app

COPY studio/web/package.json studio/web/pnpm-lock.yaml* ./
RUN pnpm install --frozen-lockfile

COPY studio/web/ .

EXPOSE 3000

CMD ["pnpm", "dev"]
