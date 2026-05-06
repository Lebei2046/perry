// Auto-generated from Perry's API manifest (#465). Do not edit by hand.
// Source: perry-api-manifest::API_MANIFEST
// Perry version: 0.5.590
// Coverage: 397 entries across 45 modules

declare module "argon2" {
  /** stdlib */
  export function hash(...args: any[]): any;
  /** stdlib */
  export function verify(...args: any[]): any;
}

declare module "async_hooks" {
}

declare module "bcrypt" {
  /** stdlib */
  export function compare(...args: any[]): any;
  /** stdlib */
  export function hash(...args: any[]): any;
}

declare module "better-sqlite3" {
  /** stdlib */
  export default function (...args: any[]): any;
}

declare module "buffer" {
  /** stdlib */
  export class Buffer { [key: string]: any; }
}

declare module "cheerio" {
  /** stdlib */
  export function load(...args: any[]): any;
}

declare module "commander" {
}

declare module "cron" {
  /** stdlib */
  export function describe(...args: any[]): any;
  /** stdlib */
  export function schedule(...args: any[]): any;
  /** stdlib */
  export function validate(...args: any[]): any;
}

declare module "crypto" {
  /** stdlib */
  export function createHash(...args: any[]): any;
  /** stdlib */
  export function createHmac(...args: any[]): any;
  /** stdlib */
  export function getRandomValues(...args: any[]): any;
  /** stdlib */
  export function md5(...args: any[]): any;
  /** stdlib */
  export function pbkdf2(...args: any[]): any;
  /** stdlib */
  export function pbkdf2Sync(...args: any[]): any;
  /** stdlib */
  export function randomBytes(...args: any[]): any;
  /** stdlib */
  export function randomUUID(...args: any[]): any;
  /** stdlib */
  export function sha256(...args: any[]): any;
}

declare module "dayjs" {
  /** stdlib */
  export function dayjs(...args: any[]): any;
  /** stdlib */
  export default function (...args: any[]): any;
}

declare module "decimal.js" {
}

declare module "dotenv" {
  /** stdlib */
  export function config(...args: any[]): any;
}

declare module "ethers" {
  /** stdlib */
  export function formatEther(...args: any[]): any;
  /** stdlib */
  export function formatUnits(...args: any[]): any;
  /** stdlib */
  export function getAddress(...args: any[]): any;
  /** stdlib */
  export function parseEther(...args: any[]): any;
  /** stdlib */
  export function parseUnits(...args: any[]): any;
}

declare module "events" {
  /** stdlib */
  export class EventEmitter { [key: string]: any; }
  /** stdlib */
  export function EventEmitter(...args: any[]): any;
}

declare module "exponential-backoff" {
  /** stdlib */
  export function backOff(...args: any[]): any;
}

declare module "fastify" {
  /** stdlib */
  export default function (...args: any[]): any;
}

declare module "ioredis" {
  /** stdlib */
  export class Redis { [key: string]: any; }
  /** stdlib */
  export function createClient(...args: any[]): any;
}

declare module "iroh" {
  /** stdlib */
  export function bind(...args: any[]): any;
}

declare module "jsonwebtoken" {
  /** stdlib */
  export function decode(...args: any[]): any;
  /** stdlib */
  export function sign(...args: any[]): any;
  /** stdlib */
  export function verify(...args: any[]): any;
}

declare module "lodash" {
  /** stdlib */
  export function camelCase(...args: any[]): any;
  /** stdlib */
  export function chunk(...args: any[]): any;
  /** stdlib */
  export function clamp(...args: any[]): any;
  /** stdlib */
  export function compact(...args: any[]): any;
  /** stdlib */
  export function drop(...args: any[]): any;
  /** stdlib */
  export function first(...args: any[]): any;
  /** stdlib */
  export function flatten(...args: any[]): any;
  /** stdlib */
  export function head(...args: any[]): any;
  /** stdlib */
  export function kebabCase(...args: any[]): any;
  /** stdlib */
  export function last(...args: any[]): any;
  /** stdlib */
  export function range(...args: any[]): any;
  /** stdlib */
  export function reverse(...args: any[]): any;
  /** stdlib */
  export function size(...args: any[]): any;
  /** stdlib */
  export function snakeCase(...args: any[]): any;
  /** stdlib */
  export function take(...args: any[]): any;
  /** stdlib */
  export function times(...args: any[]): any;
  /** stdlib */
  export function uniq(...args: any[]): any;
}

declare module "lru-cache" {
  /** stdlib */
  export default function (...args: any[]): any;
}

declare module "moment" {
  /** stdlib */
  export default function (...args: any[]): any;
  /** stdlib */
  export function moment(...args: any[]): any;
}

declare module "mongodb" {
  /** stdlib */
  export function connect(...args: any[]): any;
}

declare module "mysql2" {
  /** stdlib */
  export class Pool { [key: string]: any; }
  /** stdlib */
  export function createConnection(...args: any[]): any;
  /** stdlib */
  export function createPool(...args: any[]): any;
}

declare module "mysql2/promise" {
  /** stdlib */
  export class Pool { [key: string]: any; }
  /** stdlib */
  export function createConnection(...args: any[]): any;
  /** stdlib */
  export function createPool(...args: any[]): any;
}

declare module "nanoid" {
  /** stdlib */
  export function nanoid(...args: any[]): any;
}

declare module "net" {
  /** stdlib */
  export class Server { [key: string]: any; }
  /** stdlib */
  export class Socket { [key: string]: any; }
  /** stdlib */
  export function Socket(...args: any[]): any;
  /** stdlib */
  export function connect(...args: any[]): any;
  /** stdlib */
  export function createConnection(...args: any[]): any;
}

declare module "nodemailer" {
  /** stdlib */
  export function createTransport(...args: any[]): any;
}

declare module "os" {
  /** stdlib */
  export const EOL: any;
  /** stdlib */
  export function arch(...args: any[]): any;
  /** stdlib */
  export function cpus(...args: any[]): any;
  /** stdlib */
  export function freemem(...args: any[]): any;
  /** stdlib */
  export function homedir(...args: any[]): any;
  /** stdlib */
  export function hostname(...args: any[]): any;
  /** stdlib */
  export function networkInterfaces(...args: any[]): any;
  /** stdlib */
  export function platform(...args: any[]): any;
  /** stdlib */
  export function release(...args: any[]): any;
  /** stdlib */
  export function tmpdir(...args: any[]): any;
  /** stdlib */
  export function totalmem(...args: any[]): any;
  /** stdlib */
  export function type(...args: any[]): any;
  /** stdlib */
  export function uptime(...args: any[]): any;
  /** stdlib */
  export function userInfo(...args: any[]): any;
}

declare module "path" {
  /** stdlib */
  export const delimiter: any;
  /** stdlib */
  export const posix: any;
  /** stdlib */
  export const sep: any;
  /** stdlib */
  export const win32: any;
  /** stdlib */
  export function basename(...args: any[]): any;
  /** stdlib */
  export function dirname(...args: any[]): any;
  /** stdlib */
  export function extname(...args: any[]): any;
  /** stdlib */
  export function format(...args: any[]): any;
  /** stdlib */
  export function isAbsolute(...args: any[]): any;
  /** stdlib */
  export function join(...args: any[]): any;
  /** stdlib */
  export function normalize(...args: any[]): any;
  /** stdlib */
  export function parse(...args: any[]): any;
  /** stdlib */
  export function relative(...args: any[]): any;
  /** stdlib */
  export function resolve(...args: any[]): any;
}

declare module "perry/thread" {
  /** stdlib */
  export function parallelFilter(...args: any[]): any;
  /** stdlib */
  export function parallelMap(...args: any[]): any;
  /** stdlib */
  export function spawn(...args: any[]): any;
}

declare module "perry/tui" {
  /** stdlib */
  export function Box(...args: any[]): any;
  /** stdlib */
  export function Input(...args: any[]): any;
  /** stdlib */
  export function List(...args: any[]): any;
  /** stdlib */
  export function ProgressBar(...args: any[]): any;
  /** stdlib */
  export function Select(...args: any[]): any;
  /** stdlib */
  export function Spacer(...args: any[]): any;
  /** stdlib */
  export function Spinner(...args: any[]): any;
  /** stdlib */
  export function Text(...args: any[]): any;
  /** stdlib */
  export function TextArea(...args: any[]): any;
  /** stdlib */
  export function boxSetAlignItems(...args: any[]): any;
  /** stdlib */
  export function boxSetFlexDirection(...args: any[]): any;
  /** stdlib */
  export function boxSetFlexGrow(...args: any[]): any;
  /** stdlib */
  export function boxSetGap(...args: any[]): any;
  /** stdlib */
  export function boxSetHeight(...args: any[]): any;
  /** stdlib */
  export function boxSetJustifyContent(...args: any[]): any;
  /** stdlib */
  export function boxSetPadding(...args: any[]): any;
  /** stdlib */
  export function boxSetWidth(...args: any[]): any;
  /** stdlib */
  export function enter(...args: any[]): any;
  /** stdlib */
  export function exit(...args: any[]): any;
  /** stdlib */
  export function render(...args: any[]): any;
  /** stdlib */
  export function run(...args: any[]): any;
  /** stdlib */
  export function state(...args: any[]): any;
  /** stdlib */
  export function useInput(...args: any[]): any;
}

declare module "pg" {
  /** stdlib */
  export class Client { [key: string]: any; }
  /** stdlib */
  export class Pool { [key: string]: any; }
  /** stdlib */
  export function Pool(...args: any[]): any;
  /** stdlib */
  export function connect(...args: any[]): any;
}

declare module "process" {
  /** stdlib */
  export const arch: any;
  /** stdlib */
  export const argv: any;
  /** stdlib */
  export const env: any;
  /** stdlib */
  export const pid: any;
  /** stdlib */
  export const platform: any;
  /** stdlib */
  export const ppid: any;
  /** stdlib */
  export const stderr: any;
  /** stdlib */
  export const stdin: any;
  /** stdlib */
  export const stdout: any;
  /** stdlib */
  export const version: any;
  /** stdlib */
  export const versions: any;
}

declare module "readline" {
  /** stdlib */
  export function createInterface(...args: any[]): any;
}

declare module "sharp" {
  /** stdlib */
  export default function (...args: any[]): any;
  /** stdlib */
  export function sharp(...args: any[]): any;
}

declare module "slugify" {
  /** stdlib */
  export default function (...args: any[]): any;
  /** stdlib */
  export function slugify(...args: any[]): any;
}

declare module "tls" {
  /** stdlib */
  export function connect(...args: any[]): any;
}

declare module "tursodb" {
  /** stdlib */
  export function open(...args: any[]): any;
}

declare module "url" {
  /** stdlib */
  export class URL { [key: string]: any; }
  /** stdlib */
  export class URLSearchParams { [key: string]: any; }
}

declare module "uuid" {
  /** stdlib */
  export function v1(...args: any[]): any;
  /** stdlib */
  export function v4(...args: any[]): any;
  /** stdlib */
  export function v7(...args: any[]): any;
  /** stdlib */
  export function validate(...args: any[]): any;
}

declare module "validator" {
  /** stdlib */
  export function isEmail(...args: any[]): any;
  /** stdlib */
  export function isEmpty(...args: any[]): any;
  /** stdlib */
  export function isJSON(...args: any[]): any;
  /** stdlib */
  export function isURL(...args: any[]): any;
  /** stdlib */
  export function isUUID(...args: any[]): any;
}

declare module "worker_threads" {
  /** stdlib */
  export function getWorkerData(...args: any[]): any;
  /** stdlib */
  export function parentPort(...args: any[]): any;
  /** stdlib */
  export function workerData(...args: any[]): any;
}

declare module "ws" {
  /** stdlib */
  export class WebSocket { [key: string]: any; }
  /** stdlib */
  export class WebSocketServer { [key: string]: any; }
  /** stdlib */
  export function Server(...args: any[]): any;
  /** stdlib */
  export function WebSocket(...args: any[]): any;
  /** stdlib */
  export function closeClient(...args: any[]): any;
  /** stdlib */
  export function sendToClient(...args: any[]): any;
}

declare module "zlib" {
  /** stdlib */
  export function deflateSync(...args: any[]): any;
  /** stdlib */
  export function gunzip(...args: any[]): any;
  /** stdlib */
  export function gunzipSync(...args: any[]): any;
  /** stdlib */
  export function gzip(...args: any[]): any;
  /** stdlib */
  export function gzipSync(...args: any[]): any;
  /** stdlib */
  export function inflateSync(...args: any[]): any;
}

