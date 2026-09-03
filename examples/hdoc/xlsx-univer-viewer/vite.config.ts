import fs from 'node:fs';
import path from 'node:path';
import { defineConfig, type Plugin } from 'vite';

const contentTypes: Record<string, string> = {
  '.css': 'text/css; charset=utf-8',
  '.html': 'text/html; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
};

function hcdBundlePlugin(): Plugin {
  const configured = process.env.HCD_BUNDLE_DIR;
  const root = configured ? path.resolve(configured) : null;
  return {
    name: 'officecli-hcd-bundle',
    configureServer(server) {
      server.middlewares.use('/hcd', (request, response, next) => {
        if (!root || !request.url) return next();
        let relative: string;
        try {
          relative = decodeURIComponent(request.url.split('?', 1)[0]).replace(/^\/+/, '');
        } catch {
          response.statusCode = 400;
          response.end('Bad request');
          return;
        }
        const target = path.resolve(root, relative);
        if (target !== root && !target.startsWith(`${root}${path.sep}`)) {
          response.statusCode = 403;
          response.end('Forbidden');
          return;
        }
        let stat: fs.Stats;
        try {
          stat = fs.statSync(target);
        } catch {
          return next();
        }
        if (!stat.isFile()) return next();
        const realRoot = fs.realpathSync(root);
        const realTarget = fs.realpathSync(target);
        if (realTarget !== realRoot && !realTarget.startsWith(`${realRoot}${path.sep}`)) {
          response.statusCode = 403;
          response.end('Forbidden');
          return;
        }
        response.setHeader('Content-Type', contentTypes[path.extname(target).toLowerCase()] ?? 'application/octet-stream');
        response.setHeader('Cache-Control', 'no-store');
        fs.createReadStream(target).pipe(response);
      });
    },
  };
}

export default defineConfig({
  plugins: [hcdBundlePlugin()],
  server: { host: '127.0.0.1', port: 4174 },
  preview: { host: '127.0.0.1', port: 4174 },
});
