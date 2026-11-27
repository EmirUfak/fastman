FROM oven/bun:1-alpine

WORKDIR /app

COPY package.json ./
RUN bun install --production

COPY src ./src

EXPOSE 4010

CMD ["bun", "run", "src/index.ts"]
