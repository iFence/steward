/* Steward Node-compat polyfill (M3).
 *
 * Evaluated by the plugin runtime *after* `globalThis.steward` (the host
 * bridge) is installed and *before* the plugin's esbuild IIFE bundle. It
 * installs `require`/`module`/`exports`/`process`/`Buffer`/`global` and a small
 * module registry so the 19 most common Node built-ins resolve at runtime.
 * Pure-JS modules (`path`, `buffer`, `process`, `events`, `util`, `url`,
 * `querystring`, `string_decoder`, `assert`, `os`) are functional; network /
 * filesystem / native-backed modules (`fs`, `http`, `https`, `net`, `dns`,
 * `child_process`, `crypto`, `zlib`, `stream`) throw a clear
 * "not supported in M3" error because their host functions are not granted yet.
 *
 * `globalThis.__stewardHost` is provided by the runtime (Rust) with
 * `{ platform, arch, homedir, tmpdir, cwd, env, argv }`.
 */
(function (global) {
  "use strict";

  var host = global.__stewardHost;
  if (typeof host === "string") {
    try {
      host = JSON.parse(host);
    } catch (e) {
      host = {};
    }
  } else if (!host) {
    host = {};
  }

  /* -------------------------------------------------------------- helpers */
  function utf8Encode(str) {
    var out = [];
    for (var i = 0; i < str.length; i++) {
      var c = str.charCodeAt(i);
      if (c < 0x80) {
        out.push(c);
      } else if (c < 0x800) {
        out.push(0xc0 | (c >> 6), 0x80 | (c & 63));
      } else if (c >= 0xd800 && c <= 0xdbff && i + 1 < str.length) {
        var c2 = str.charCodeAt(i + 1);
        if (c2 >= 0xdc00 && c2 <= 0xdfff) {
          var cp = 0x10000 + ((c - 0xd800) << 10) + (c2 - 0xdc00);
          out.push(
            0xf0 | (cp >> 18),
            0x80 | ((cp >> 12) & 63),
            0x80 | ((cp >> 6) & 63),
            0x80 | (cp & 63),
          );
          i++;
        } else {
          out.push(0xef, 0xbf, 0xbd);
        }
      } else if (c < 0x10000) {
        out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
      } else {
        out.push(0xef, 0xbf, 0xbd);
      }
    }
    return new Uint8Array(out);
  }

  function utf8Decode(bytes) {
    var out = "";
    var i = 0;
    while (i < bytes.length) {
      var b = bytes[i];
      var cp;
      var len;
      if (b < 0x80) {
        cp = b;
        len = 1;
      } else if ((b & 0xe0) === 0xc0) {
        cp = b & 0x1f;
        len = 2;
      } else if ((b & 0xf0) === 0xe0) {
        cp = b & 0x0f;
        len = 3;
      } else if ((b & 0xf8) === 0xf0) {
        cp = b & 0x07;
        len = 4;
      } else {
        out += "\ufffd";
        i++;
        continue;
      }
      if (i + len > bytes.length) {
        out += "\ufffd";
        break;
      }
      for (var k = 1; k < len; k++) {
        cp = (cp << 6) | (bytes[i + k] & 0x3f);
      }
      i += len;
      if (cp >= 0x10000) {
        var a = cp - 0x10000;
        out += String.fromCharCode(0xd800 | (a >> 10), 0xdc00 | (a & 0x3ff));
      } else {
        out += String.fromCharCode(cp);
      }
    }
    return out;
  }

  function hexEncode(bytes) {
    var out = "";
    for (var i = 0; i < bytes.length; i++) {
      out += ("0" + bytes[i].toString(16)).slice(-2);
    }
    return out;
  }

  function hexDecode(hex) {
    var out = [];
    for (var i = 0; i + 1 < hex.length; i += 2) {
      out.push(parseInt(hex.substr(i, 2), 16));
    }
    return new Uint8Array(out);
  }

  var B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  function base64Encode(bytes) {
    var out = "";
    for (var i = 0; i < bytes.length; i += 3) {
      var b0 = bytes[i];
      var b1 = i + 1 < bytes.length ? bytes[i + 1] : 0;
      var b2 = i + 2 < bytes.length ? bytes[i + 2] : 0;
      out += B64[b0 >> 2];
      out += B64[((b0 & 3) << 4) | (b1 >> 4)];
      out += i + 1 < bytes.length ? B64[((b1 & 15) << 2) | (b2 >> 6)] : "=";
      out += i + 2 < bytes.length ? B64[b2 & 63] : "=";
    }
    return out;
  }
  function base64Decode(text) {
    text = text.replace(/[^A-Za-z0-9+/=]/g, "");
    var out = [];
    for (var i = 0; i < text.length; i += 4) {
      var a = B64.indexOf(text[i]);
      var b = B64.indexOf(text[i + 1]);
      var c = text[i + 2] === "=" ? 0 : B64.indexOf(text[i + 2]);
      var d = text[i + 3] === "=" ? 0 : B64.indexOf(text[i + 3]);
      out.push((a << 2) | (b >> 4));
      if (text[i + 2] !== "=") out.push(((b & 15) << 4) | (c >> 2));
      if (text[i + 3] !== "=") out.push(((c & 3) << 6) | d);
    }
    return new Uint8Array(out);
  }

  /* ------------------------------------------------------------ globals */
  global.global = global;
  global.globalThis = global;
  global.__dirname = ".";
  global.__filename = "index.js";
  global.exports = {};
  global.module = { exports: global.exports };

  var env = {};
  var sourceEnv = host.env || {};
  if (typeof sourceEnv === "object") {
    for (var key in sourceEnv) {
      if (Object.prototype.hasOwnProperty.call(sourceEnv, key)) env[key] = String(sourceEnv[key]);
    }
  }
  global.process = {
    env: env,
    argv: (host.argv || ["steward"]).slice(),
    platform: host.platform || "",
    arch: host.arch || "",
    version: "",
    versions: {},
    cwd: function () {
      return host.cwd || ".";
    },
    nextTick: function (cb) {
      Promise.resolve().then(cb);
    },
    exit: function () {},
    umask: function () {
      return 0;
    },
    pid: 0,
    hrtime: function () {
      return [0, 0];
    },
    title: "steward",
  };

  global.console = {
    log: function () {},
    info: function () {},
    warn: function () {},
    error: function () {},
    debug: function () {},
  };

  /* ------------------------------------------------------------- buffer */
  function Buffer(arg, encoding) {
    var buf;
    if (typeof arg === "number") {
      buf = new Uint8Array(arg);
    } else if (typeof arg === "string") {
      return Buffer.from(arg, encoding);
    } else if (arg instanceof Uint8Array) {
      buf = new Uint8Array(arg);
    } else if (Array.isArray(arg)) {
      buf = new Uint8Array(arg);
    } else {
      buf = new Uint8Array(0);
    }
    attachBufferMethods(buf);
    return buf;
  }
  Buffer.isBuffer = function (b) {
    return b instanceof Uint8Array && b._stewardBuffer === true;
  };
  Buffer.isEncoding = function (enc) {
    return ["utf8", "utf-8", "ascii", "latin1", "binary", "hex", "base64", "ucs2", "utf16le"].includes(
      String(enc).toLowerCase(),
    );
  };
  Buffer.byteLength = function (value, encoding) {
    if (typeof value !== "string") value = String(value);
    var enc = String(encoding || "utf8").toLowerCase();
    if (enc === "hex") return hexDecode(value).length;
    if (enc === "base64") return base64Decode(value).length;
    return utf8Encode(value).length;
  };
  Buffer.from = function (value, encoding) {
    if (typeof value === "string") {
      var enc = String(encoding || "utf8").toLowerCase();
      if (enc === "hex") return Buffer(hexDecode(value));
      if (enc === "base64") return Buffer(base64Decode(value));
      if (enc === "ascii" || enc === "latin1" || enc === "binary") {
        var out = [];
        for (var i = 0; i < value.length; i++) out.push(value.charCodeAt(i) & 0xff);
        return Buffer(new Uint8Array(out));
      }
      return Buffer(utf8Encode(value));
    }
    if (value instanceof Buffer) return Buffer(value);
    if (value instanceof Uint8Array) return Buffer(new Uint8Array(value));
    if (Array.isArray(value)) return Buffer(new Uint8Array(value));
    return Buffer(new Uint8Array(0));
  };
  Buffer.alloc = function (size, fill, encoding) {
    var buf = new Uint8Array(size);
    if (fill !== undefined && fill !== 0) {
      var f = Buffer.from(fill, encoding);
      for (var i = 0; i < size; i++) buf[i] = f[i % f.length];
    }
    return Buffer(buf);
  };
  Buffer.allocUnsafe = function (size) {
    return Buffer(new Uint8Array(size));
  };
  Buffer.concat = function (list, totalLength) {
    var total = totalLength || 0;
    if (!total) {
      for (var i = 0; i < list.length; i++) total += list[i].length;
    }
    var out = new Uint8Array(total);
    var offset = 0;
    for (var j = 0; j < list.length; j++) {
      out.set(list[j], offset);
      offset += list[j].length;
    }
    return Buffer(out);
  };
  Buffer.compare = function (a, b) {
    var len = Math.min(a.length, b.length);
    for (var i = 0; i < len; i++) {
      if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1;
    }
    return a.length < b.length ? -1 : a.length > b.length ? 1 : 0;
  };
  global.Buffer = Buffer;

  function attachBufferMethods(buf) {
    Object.defineProperty(buf, "_stewardBuffer", { value: true });
    function decodeUtf16le(bytes) {
      var out = "";
      for (var i = 0; i + 1 < bytes.length; i += 2) {
        out += String.fromCharCode(bytes[i] | (bytes[i + 1] << 8));
      }
      return out;
    }
    buf.toString = function (encoding, start, end) {
      var enc = String(encoding || "utf8").toLowerCase();
      var s = start || 0;
      var e = end === undefined ? this.length : end;
      var slice = this.subarray(s, e);
      if (enc === "hex") return hexEncode(slice);
      if (enc === "base64") return base64Encode(slice);
      if (enc === "utf16le" || enc === "ucs2") return decodeUtf16le(slice);
      if (enc === "ascii" || enc === "latin1" || enc === "binary") {
        var out = "";
        for (var i = 0; i < slice.length; i++) out += String.fromCharCode(slice[i] & 0xff);
        return out;
      }
      return utf8Decode(slice);
    };
    buf.toJSON = function () {
      return { type: "Buffer", data: Array.prototype.slice.call(this) };
    };
    buf.equals = function (other) {
      return Buffer.compare(this, other) === 0;
    };
    buf.slice = function (start, end) {
      return Buffer(this.subarray(start, end));
    };
    buf.indexOf = function (needle, offset) {
      var n = Buffer.from(needle);
      for (var i = offset || 0; i + n.length <= this.length; i++) {
        var ok = true;
        for (var j = 0; j < n.length; j++) {
          if (this[i + j] !== n[j]) {
            ok = false;
            break;
          }
        }
        if (ok) return i;
      }
      return -1;
    };
    buf.includes = function (needle, offset) {
      return this.indexOf(needle, offset) !== -1;
    };
    buf.readUInt8 = function (offset) {
      return this[offset];
    };
    buf.writeUInt8 = function (value, offset) {
      this[offset] = value & 0xff;
      return offset + 1;
    };
    buf.readUInt16LE = function (offset) {
      return this[offset] | (this[offset + 1] << 8);
    };
    buf.readUInt16BE = function (offset) {
      return (this[offset] << 8) | this[offset + 1];
    };
    buf.readUInt32LE = function (offset) {
      return (
        (this[offset] | (this[offset + 1] << 8) | (this[offset + 2] << 16) |
          (this[offset + 3] << 24)) >>>
        0
      );
    };
    buf.readUInt32BE = function (offset) {
      return (
        ((this[offset] << 24) | (this[offset + 1] << 16) | (this[offset + 2] << 8) |
          this[offset + 3]) >>>
        0
      );
    };
    buf.writeUInt16LE = function (value, offset) {
      this[offset] = value & 0xff;
      this[offset + 1] = (value >> 8) & 0xff;
      return offset + 2;
    };
    buf.writeUInt16BE = function (value, offset) {
      this[offset] = (value >> 8) & 0xff;
      this[offset + 1] = value & 0xff;
      return offset + 2;
    };
    buf.writeUInt32LE = function (value, offset) {
      this[offset] = value & 0xff;
      this[offset + 1] = (value >> 8) & 0xff;
      this[offset + 2] = (value >> 16) & 0xff;
      this[offset + 3] = (value >> 24) & 0xff;
      return offset + 4;
    };
    buf.writeUInt32BE = function (value, offset) {
      this[offset] = (value >> 24) & 0xff;
      this[offset + 1] = (value >> 16) & 0xff;
      this[offset + 2] = (value >> 8) & 0xff;
      this[offset + 3] = value & 0xff;
      return offset + 4;
    };
  }

  /* ----------------------------------------------------- module system */
  var factories = Object.create(null);
  var cache = Object.create(null);
  function define(name, factory) {
    factories[name] = factory;
  }
  function requireModule(id) {
    var name = id;
    if (name.indexOf("node:") === 0) name = name.slice(5);
    if (!(name in factories)) {
      throw new Error("Cannot find module '" + id + "'");
    }
    if (cache[name]) return cache[name].exports;
    var mod = { exports: {} };
    cache[name] = mod;
    factories[name](mod.exports, mod, requireModule);
    return mod.exports;
  }
  global.require = requireModule;

  function stub(name) {
    define(name, function () {
      throw new Error("Steward: '" + name + "' is not supported in M3");
    });
  }

  /* ---------------------------------------------------------------- path */
  define("path", function (exports) {
    var sep = "/";
    var delimiter = ":";
    function isAbs(p) {
      return /^\//.test(p) || /^[A-Za-z]:[\\/]/.test(p);
    }
    function normalize(p) {
      p = String(p).replace(/\\/g, "/");
      var absolute = isAbs(p);
      var parts = p.split("/");
      var out = [];
      for (var i = 0; i < parts.length; i++) {
        var part = parts[i];
        if (!part || part === ".") continue;
        if (part === "..") {
          if (out.length && out[out.length - 1] !== "..") out.pop();
          else if (!absolute) out.push("..");
        } else {
          out.push(part);
        }
      }
      var result = (absolute ? "/" : "") + out.join("/");
      if (!result) result = ".";
      return result;
    }
    function join() {
      var parts = Array.prototype.slice.call(arguments).filter(function (p) {
        return p !== "" && p !== undefined && p !== null;
      });
      return normalize(parts.join("/"));
    }
    function resolve() {
      var resolved = "";
      var parts = Array.prototype.slice.call(arguments);
      for (var i = parts.length - 1; i >= 0; i--) {
        var p = String(parts[i]);
        if (!p) continue;
        resolved = p + "/" + resolved;
        if (isAbs(p)) break;
      }
      return normalize(resolved);
    }
    function dirname(p) {
      p = String(p).replace(/\\/g, "/");
      if (!p) return ".";
      var idx = p.lastIndexOf("/");
      return idx < 0 ? "." : idx === 0 ? "/" : p.slice(0, idx);
    }
    function basename(p, ext) {
      p = String(p).replace(/\\/g, "/");
      var base = p.slice(p.lastIndexOf("/") + 1);
      if (ext && base.endsWith(ext)) base = base.slice(0, -ext.length);
      return base;
    }
    function extname(p) {
      p = String(p).replace(/\\/g, "/");
      var base = basename(p);
      var idx = base.lastIndexOf(".");
      return idx <= 0 ? "" : base.slice(idx);
    }
    function relative(from, to) {
      var a = normalize(from).split("/");
      var b = normalize(to).split("/");
      while (a.length && b.length && a[0] === b[0]) {
        a.shift();
        b.shift();
      }
      var dots = a
        .filter(function (p) {
          return p && p !== ".";
        })
        .map(function () {
          return "..";
        });
      return dots.concat(b).join("/") || ".";
    }
    Object.assign(exports, {
      sep: sep,
      delimiter: delimiter,
      posix: {
        sep: "/",
        delimiter: ":",
        join: join,
        resolve: resolve,
        normalize: normalize,
        dirname: dirname,
        basename: basename,
        extname: extname,
        relative: relative,
        isAbsolute: isAbs,
      },
      win32: {
        sep: "\\",
        delimiter: ";",
        join: join,
        resolve: resolve,
        normalize: normalize,
        dirname: dirname,
        basename: basename,
        extname: extname,
        relative: relative,
        isAbsolute: isAbs,
      },
    });
    exports.join = join;
    exports.resolve = resolve;
    exports.normalize = normalize;
    exports.dirname = dirname;
    exports.basename = basename;
    exports.extname = extname;
    exports.relative = relative;
    exports.isAbsolute = isAbs;
    exports.parse = function (p) {
      var root = "";
      var dir = dirname(p);
      if (dir === ".") dir = "";
      return {
        root: root,
        dir: dir,
        base: basename(p),
        ext: extname(p),
        name: basename(p, extname(p)),
      };
    };
  });

  /* -------------------------------------------------------------- events */
  define("events", function (exports) {
    function EventEmitter() {
      this._events = Object.create(null);
    }
    EventEmitter.prototype.on = function (name, listener) {
      var arr = this._events[name] || (this._events[name] = []);
      arr.push(listener);
      return this;
    };
    EventEmitter.prototype.addListener = EventEmitter.prototype.on;
    EventEmitter.prototype.prependListener = function (name, listener) {
      var arr = this._events[name] || (this._events[name] = []);
      arr.unshift(listener);
      return this;
    };
    EventEmitter.prototype.once = function (name, listener) {
      var self = this;
      function wrapper() {
        self.removeListener(name, wrapper);
        listener.apply(self, arguments);
      }
      wrapper.listener = listener;
      return this.on(name, wrapper);
    };
    EventEmitter.prototype.emit = function (name) {
      var args = Array.prototype.slice.call(arguments, 1);
      var arr = this._events[name];
      if (!arr || !arr.length) return false;
      for (var i = 0; i < arr.length; i++) arr[i].apply(this, args);
      return true;
    };
    EventEmitter.prototype.removeListener = function (name, listener) {
      var arr = this._events[name];
      if (arr) {
        this._events[name] = arr.filter(function (fn) {
          return fn !== listener && fn !== listener.listener;
        });
      }
      return this;
    };
    EventEmitter.prototype.removeAllListeners = function (name) {
      if (name === undefined) this._events = Object.create(null);
      else delete this._events[name];
      return this;
    };
    EventEmitter.prototype.listeners = function (name) {
      return (this._events[name] || []).slice();
    };
    EventEmitter.prototype.listenerCount = function (name) {
      return (this._events[name] || []).length;
    };
    EventEmitter.prototype.setMaxListeners = function () {
      return this;
    };
    exports.EventEmitter = EventEmitter;
    exports.once = function (emitter, name) {
      return new Promise(function (resolve) {
        emitter.once(name, function () {
          resolve(Array.prototype.slice.call(arguments));
        });
      });
    };
  });

  /* ---------------------------------------------------------------- util */
  define("util", function (exports) {
    exports.format = function () {
      var args = Array.prototype.slice.call(arguments);
      var fmt = args.shift();
      if (typeof fmt !== "string") return args.map(String).join(" ");
      var i = 0;
      return fmt.replace(/%[sdjif%]/g, function (m) {
        if (m === "%%") return "%";
        var val = args[i++];
        if (m === "%s") return String(val);
        if (m === "%d") return String(Number(val));
        if (m === "%j") return JSON.stringify(val);
        return String(val);
      });
    };
    exports.inspect = function (value) {
      return String(value);
    };
    exports.inherits = function (ctor, superCtor) {
      ctor.super_ = superCtor;
      ctor.prototype = Object.create(superCtor.prototype, {
        constructor: { value: ctor, enumerable: false, writable: true, configurable: true },
      });
    };
    exports.promisify = function (fn) {
      return function () {
        var args = Array.prototype.slice.call(arguments);
        var self = this;
        return new Promise(function (resolve, reject) {
          args.push(function (err, value) {
            if (err) reject(err);
            else resolve(value);
          });
          fn.apply(self, args);
        });
      };
    };
    exports.deprecate = function (fn) {
      return fn;
    };
    exports.isDeepStrictEqual = function (a, b) {
      return JSON.stringify(a) === JSON.stringify(b);
    };
    exports.types = {};
  });

  /* -------------------------------------------------------- querystring */
  define("querystring", function (exports) {
    function parse(str, sep, eq) {
      sep = sep || "&";
      eq = eq || "=";
      var out = {};
      var parts = String(str).split(sep);
      for (var i = 0; i < parts.length; i++) {
        var item = parts[i];
        if (!item) continue;
        var idx = item.indexOf(eq);
        var k, v;
        if (idx < 0) {
          k = decodeURIComponent(item.replace(/\+/g, " "));
          v = "";
        } else {
          k = decodeURIComponent(item.slice(0, idx).replace(/\+/g, " "));
          v = decodeURIComponent(item.slice(idx + 1).replace(/\+/g, " "));
        }
        out[k] = v;
      }
      return out;
    }
    function stringify(obj) {
      var parts = [];
      for (var k in obj) {
        if (Object.prototype.hasOwnProperty.call(obj, k)) {
          var v = obj[k];
          if (Array.isArray(v)) {
            for (var i = 0; i < v.length; i++) {
              parts.push(encodeURIComponent(k) + "=" + encodeURIComponent(String(v[i])));
            }
          } else {
            parts.push(encodeURIComponent(k) + "=" + encodeURIComponent(String(v)));
          }
        }
      }
      return parts.join("&");
    }
    exports.parse = parse;
    exports.stringify = stringify;
    exports.escape = encodeURIComponent;
    exports.unescape = decodeURIComponent;
  });

  /* ----------------------------------------------------- string_decoder */
  define("string_decoder", function (exports) {
    function StringDecoder(encoding) {
      this.encoding = encoding || "utf8";
    }
    StringDecoder.prototype.write = function (buf) {
      return Buffer.from(buf).toString(this.encoding);
    };
    StringDecoder.prototype.end = function (buf) {
      return buf ? this.write(buf) : "";
    };
    exports.StringDecoder = StringDecoder;
  });

  /* ---------------------------------------------------------------- assert */
  define("assert", function (exports, mod) {
    function AssertionError(message) {
      this.name = "AssertionError";
      this.message = message || "";
    }
    AssertionError.prototype = Object.create(Error.prototype);
    AssertionError.prototype.constructor = AssertionError;
    function ok(value, message) {
      if (!value) {
        throw new AssertionError(message || "failed assertion");
      }
    }
    function equal(a, b, message) {
      if (a != b) throw new AssertionError(message || "values are not loosely equal");
    }
    function strictEqual(a, b, message) {
      if (a !== b) throw new AssertionError(message || "values are not strictly equal");
    }
    function deepEqual(a, b, message) {
      if (JSON.stringify(a) !== JSON.stringify(b)) {
        throw new AssertionError(message || "values are not deeply equal");
      }
    }
    function fail(message) {
      throw new AssertionError(message || "failed");
    }
    ok.ok = ok;
    ok.equal = equal;
    ok.strictEqual = strictEqual;
    ok.deepEqual = deepEqual;
    ok.deepStrictEqual = deepEqual;
    ok.notEqual = function (a, b, m) {
      if (a == b) throw new AssertionError(m || "values are equal");
    };
    ok.notStrictEqual = function (a, b, m) {
      if (a === b) throw new AssertionError(m || "values are strictly equal");
    };
    ok.fail = fail;
    ok.AssertionError = AssertionError;
    ok.strict = ok;
    mod.exports = ok;
  });

  /* ------------------------------------------------------------------- os */
  define("os", function (exports) {
    exports.EOL = host.platform === "win32" ? "\r\n" : "\n";
    exports.platform = function () {
      return host.platform || "";
    };
    exports.arch = function () {
      return host.arch || "";
    };
    exports.homedir = function () {
      return host.homedir || ".";
    };
    exports.tmpdir = function () {
      return host.tmpdir || ".";
    };
    exports.cpus = function () {
      return [{ model: "Steward", speed: 0, times: {} }];
    };
    exports.totalmem = function () {
      return 0;
    };
    exports.freemem = function () {
      return 0;
    };
    exports.hostname = function () {
      return "steward";
    };
    exports.type = function () {
      return "Steward";
    };
    exports.release = function () {
      return "0.1.0";
    };
    exports.networkInterfaces = function () {
      return {};
    };
  });

  /* ------------------------------------------------------------------ url */
  define("url", function (exports) {
    function URL(input, base) {
      var raw = String(input);
      if (base) raw = resolveUrl(base) + raw;
      var m = /^([a-zA-Z][a-zA-Z0-9+.-]*):(\/\/[^/?#]*)?([^?#]*)(\?[^#]*)?(#.*)?$/.exec(raw);
      if (!m) throw new Error("Invalid URL: " + raw);
      this.protocol = (m[1] || "") + ":";
      var authority = (m[2] || "").slice(2);
      var at = authority.lastIndexOf("@");
      if (at >= 0) {
        this.username = authority.slice(0, at);
        authority = authority.slice(at + 1);
      }
      var colon = authority.lastIndexOf(":");
      if (colon >= 0) {
        this.hostname = authority.slice(0, colon);
        this.port = authority.slice(colon + 1);
      } else {
        this.hostname = authority;
        this.port = "";
      }
      var hostport = this.hostname + (this.port ? ":" + this.port : "");
      this.host = hostport;
      this.pathname = m[3] || "/";
      this.search = m[4] || "";
      this.hash = m[5] || "";
      this.href = this.protocol + "//" + hostport + this.pathname + this.search + this.hash;
      this.searchParams = new URLSearchParams((this.search || "").slice(1));
    }
    URL.prototype.toString = function () {
      return this.href;
    };
    URL.prototype.toJSON = function () {
      return this.href;
    };
    Object.defineProperty(URL.prototype, "origin", {
      get: function () {
        return this.protocol + "//" + this.host;
      },
    });
    function URLSearchParams(input) {
      this._params = {};
      if (typeof input === "string") {
        var query = requireModule("querystring");
        this._params = query.parse(input);
      }
    }
    URLSearchParams.prototype.get = function (name) {
      return Object.prototype.hasOwnProperty.call(this._params, name) ? this._params[name] : null;
    };
    URLSearchParams.prototype.getAll = function (name) {
      var v = this._params[name];
      return v === undefined ? [] : Array.isArray(v) ? v : [v];
    };
    URLSearchParams.prototype.has = function (name) {
      return Object.prototype.hasOwnProperty.call(this._params, name);
    };
    URLSearchParams.prototype.toString = function () {
      return requireModule("querystring").stringify(this._params);
    };
    URLSearchParams.prototype.append = function (k, v) {
      this._params[k] = this._params[k] === undefined ? String(v) : String(this._params[k]) + "," + v;
    };
    URLSearchParams.prototype.set = function (k, v) {
      this._params[k] = String(v);
    };
    function resolveUrl(baseUrl) {
      var m = /^([a-zA-Z][a-zA-Z0-9+.-]*:\/\/[^/?#]*)\//.exec(String(baseUrl));
      return m ? m[1] + "/" : String(baseUrl).replace(/[^/]*$/, "");
    }
    exports.URL = URL;
    exports.URLSearchParams = URLSearchParams;
    exports.format = function (obj) {
      return (
        obj.protocol +
        "//" +
        (obj.auth ? obj.auth + "@" : "") +
        obj.hostname +
        (obj.port ? ":" + obj.port : "") +
        (obj.pathname || "/") +
        (obj.search || "")
      );
    };
    exports.parse = function (str) {
      var u = new URL(str);
      return {
        protocol: u.protocol,
        hostname: u.hostname,
        port: u.port,
        pathname: u.pathname,
        search: u.search,
        hash: u.hash,
      };
    };
    exports.resolve = resolveUrl;
  });

  /* Async host bridge: a plugin's `await` can park on a cross-process host
   * request (e.g. `host.fs.read`). The JS side owns the resolver registry
   * (`__stewardAsync`) and the Rust runtime resumes the parked promise when the
   * host replies (see `handle_host_response` in the runtime). */
  var asyncResolvers = Object.create(null);
  var asyncSeq = 0;
  global.__stewardAsync = asyncResolvers;
  function hostRequest(method, params) {
    var pendingId = ++asyncSeq;
    return new Promise(function (resolve, reject) {
      asyncResolvers[String(pendingId)] = { resolve: resolve, reject: reject };
      try {
        global.steward.__hostSend(method, params, pendingId);
      } catch (e) {
        delete asyncResolvers[String(pendingId)];
        reject(e);
      }
    });
  }

  /* `fs.readFile` is backed by a host `host.fs.read` round-trip; other Node
   * fs methods remain unsupported in this phase. */
  var fsModule = {
    readFile: function (path, encoding) {
      var enc = encoding === undefined || encoding === null ? "utf8" : String(encoding);
      return hostRequest("host.fs.read", { path: String(path), encoding: enc }).then(function (res) {
        if (res && res.base64) {
          return Buffer.from(res.data, "base64");
        }
        return res ? res.data : "";
      });
    },
    writeFile: function (path, data, encoding) {
      var enc = encoding === undefined || encoding === null ? "utf8" : String(encoding);
      var payload;
      var base64 = false;
      if (enc === "base64") {
        base64 = true;
        payload = data instanceof Uint8Array ? base64Encode(data) : String(data);
      } else {
        payload = String(data);
      }
      return hostRequest("host.fs.write", {
        path: String(path),
        data: payload,
        base64: base64,
      }).then(function () {
        return undefined;
      });
    },
  };
  function fsUnsupported(name) {
    throw new Error("Steward: 'fs." + name + "' is not supported in this phase");
  }
  ["readFileSync", "writeFileSync", "readdir", "readdirSync", "stat", "statSync",
    "existsSync", "mkdir", "mkdirSync", "rm", "rmSync", "unlink", "appendFile"].forEach(function (name) {
    fsModule[name] = function () {
      fsUnsupported(name);
    };
  });
  define("fs", function (exports) {
    exports.readFile = fsModule.readFile;
    exports.writeFile = fsModule.writeFile;
    ["readFileSync", "writeFileSync", "readdir", "readdirSync", "stat", "statSync",
      "existsSync", "mkdir", "mkdirSync", "rm", "rmSync", "unlink", "appendFile"].forEach(function (name) {
      exports[name] = fsModule[name];
    });
  });
  if (global.steward) {
    global.steward.fs = { readFile: fsModule.readFile, writeFile: fsModule.writeFile };
  }

  /* Remaining network/native modules the runtime cannot back in this phase. */
  ["http", "https", "net", "dns", "child_process", "crypto", "zlib", "stream"].forEach(stub);
})(globalThis);
