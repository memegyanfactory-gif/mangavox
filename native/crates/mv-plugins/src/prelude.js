(function (g) {
  'use strict';
  var enc = encodeURIComponent;
  function qdec(s) { try { return decodeURIComponent(String(s).replace(/\+/g, ' ')) } catch (e) { return s } }
  function qenc(s) { return enc(s).replace(/%20/g, '+').replace(/[!'()~]/g, function (c) { return '%' + c.charCodeAt(0).toString(16).toUpperCase() }) }

  function URLSearchParams(init) {
    this._l = []
    if (init == null) return
    if (typeof init === 'object' && init instanceof URLSearchParams) { this._l = init._l.map(function (p) { return [p[0], p[1]] }); return }
    if (Array.isArray(init)) { for (var i = 0; i < init.length; i++) this._l.push([String(init[i][0]), String(init[i][1])]); return }
    if (typeof init === 'object') { for (var k in init) this._l.push([k, String(init[k])]); return }
    var s = String(init); if (s.charAt(0) === '?') s = s.slice(1)
    if (!s) return
    var parts = s.split('&')
    for (var j = 0; j < parts.length; j++) {
      if (!parts[j]) continue
      var eq = parts[j].indexOf('=')
      if (eq < 0) this._l.push([qdec(parts[j]), ''])
      else this._l.push([qdec(parts[j].slice(0, eq)), qdec(parts[j].slice(eq + 1))])
    }
  }
  var P = URLSearchParams.prototype
  P.append = function (k, v) { this._l.push([String(k), String(v)]); this._u && this._u() }
  P.delete = function (k) { this._l = this._l.filter(function (p) { return p[0] !== k }); this._u && this._u() }
  P.get = function (k) { for (var i = 0; i < this._l.length; i++) if (this._l[i][0] === k) return this._l[i][1]; return null }
  P.getAll = function (k) { return this._l.filter(function (p) { return p[0] === k }).map(function (p) { return p[1] }) }
  P.has = function (k) { return this.get(k) !== null }
  P.set = function (k, v) {
    k = String(k); v = String(v)
    var found = false, out = []
    for (var i = 0; i < this._l.length; i++) {
      if (this._l[i][0] === k) { if (!found) { out.push([k, v]); found = true } } else out.push(this._l[i])
    }
    if (!found) out.push([k, v])
    this._l = out; this._u && this._u()
  }
  P.sort = function () { this._l.sort(function (a, b) { return a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0 }); this._u && this._u() }
  P.forEach = function (cb, t) { for (var i = 0; i < this._l.length; i++) cb.call(t, this._l[i][1], this._l[i][0], this) }
  P.keys = function () { return this._l.map(function (p) { return p[0] })[Symbol.iterator]() }
  P.values = function () { return this._l.map(function (p) { return p[1] })[Symbol.iterator]() }
  P.entries = function () { return this._l.map(function (p) { return [p[0], p[1]] })[Symbol.iterator]() }
  P[Symbol.iterator] = P.entries
  P.toString = function () { return this._l.map(function (p) { return qenc(p[0]) + '=' + qenc(p[1]) }).join('&') }

  var ABS = /^([a-zA-Z][a-zA-Z0-9+.\-]*:)\/\/([^\/?#]*)([^?#]*)(\?[^#]*)?(#[\s\S]*)?$/
  function normPath(p) {
    var segs = p.split('/'), out = []
    for (var i = 0; i < segs.length; i++) {
      var s = segs[i]
      if (s === '..') { if (out.length > 1) out.pop() } else if (s !== '.') out.push(s)
    }
    var last = segs[segs.length - 1]
    if ((last === '.' || last === '..') && out[out.length - 1] !== '') out.push('')
    return out.join('/')
  }
  function parse(url, base) {
    url = String(url).trim()
    var m = ABS.exec(url)
    if (!m) {
      if (base === undefined) throw new TypeError('Invalid URL: ' + url)
      var b = ABS.exec(typeof base === 'string' ? base : base.href)
      if (!b) throw new TypeError('Invalid base URL')
      var proto = b[1], host = b[2], path = b[3] || '/', search = b[4] || '', hash = ''
      if (url.slice(0, 2) === '//') return parse(proto + url)
      var hi = url.indexOf('#'); if (hi >= 0) { hash = url.slice(hi); url = url.slice(0, hi) }
      if (url !== '') {
        var qi = url.indexOf('?'), q = ''
        if (qi >= 0) { q = url.slice(qi); url = url.slice(0, qi) }
        search = q
        if (url !== '') {
          if (url.charAt(0) === '/') path = url
          else path = path.slice(0, path.lastIndexOf('/') + 1) + url
        }
      }
      return parse(proto + '//' + host + normPath(path) + search + hash)
    }
    var auth = m[2], hostport = auth.slice(auth.lastIndexOf('@') + 1)
    var pm = /^(.*?)(?::(\d*))?$/.exec(hostport)
    var hostname = pm[1].toLowerCase(), port = pm[2] || ''
    if ((m[1] === 'http:' && port === '80') || (m[1] === 'https:' && port === '443')) port = ''
    return { protocol: m[1].toLowerCase(), hostname: hostname, port: port, pathname: normPath(m[3] || '/'), search: m[4] && m[4] !== '?' ? m[4] : '', hash: m[5] && m[5] !== '#' ? m[5] : '' }
  }
  function URL(url, base) {
    var p = parse(url, base), self = this
    this.protocol = p.protocol; this.hostname = p.hostname; this.port = p.port
    this.pathname = p.pathname; this.hash = p.hash; this.username = ''; this.password = ''
    this._search = p.search
    this.searchParams = new URLSearchParams(p.search)
    this.searchParams._u = function () { var s = self.searchParams.toString(); self._search = s ? '?' + s : '' }
  }
  Object.defineProperties(URL.prototype, {
    search: { get: function () { return this._search }, set: function (v) { v = String(v); this._search = v && v.charAt(0) !== '?' ? '?' + v : v; var u = this.searchParams._u; this.searchParams = new URLSearchParams(this._search); this.searchParams._u = u } },
    host: { get: function () { return this.hostname + (this.port ? ':' + this.port : '') } },
    origin: { get: function () { return this.protocol + '//' + this.host } },
    href: { get: function () { return this.protocol + '//' + this.host + this.pathname + this._search + this.hash } }
  })
  URL.prototype.toString = function () { return this.href }
  URL.prototype.toJSON = function () { return this.href }

  var B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'
  g.btoa = function (s) {
    s = String(s); var o = ''
    for (var i = 0; i < s.length; i += 3) {
      var a = s.charCodeAt(i), b = s.charCodeAt(i + 1), c = s.charCodeAt(i + 2)
      if (a > 255 || b > 255 || c > 255) throw new Error('InvalidCharacterError')
      var n = (a << 16) | ((b || 0) << 8) | (c || 0)
      o += B64.charAt(n >> 18 & 63) + B64.charAt(n >> 12 & 63) + (i + 1 < s.length ? B64.charAt(n >> 6 & 63) : '=') + (i + 2 < s.length ? B64.charAt(n & 63) : '=')
    }
    return o
  }
  g.atob = function (s) {
    s = String(s).replace(/[\s=]+/g, ''); var o = '', bits = 0, acc = 0
    for (var i = 0; i < s.length; i++) {
      var v = B64.indexOf(s.charAt(i)); if (v < 0) throw new Error('InvalidCharacterError')
      acc = ((acc << 6) | v) & 0xffffff; bits += 6
      if (bits >= 8) { bits -= 8; o += String.fromCharCode((acc >> bits) & 255) }
    }
    return o
  }

  function fmt(args) {
    return Array.prototype.map.call(args, function (a) {
      if (typeof a === 'string') return a
      if (a instanceof Error) return a.stack || a.message
      try { return JSON.stringify(a) } catch (e) { return String(a) }
    }).join(' ')
  }
  g.console = {
    log: function () { __log('log', fmt(arguments)) },
    info: function () { __log('log', fmt(arguments)) },
    debug: function () { __log('log', fmt(arguments)) },
    warn: function () { __log('warn', fmt(arguments)) },
    error: function () { __log('error', fmt(arguments)) }
  }

  g.URL = URL; g.URLSearchParams = URLSearchParams

  var rawFetch = g.__fetch_raw, rawRender = g.__render_raw, rawSolve = g.__solve_raw
  g.__fetch = async function (url, options) {
    var o = options && typeof options === 'object' ? options : {}
    var r = JSON.parse(await rawFetch(String(url), JSON.stringify(o)))
    if (r.error) throw new Error(r.error)
    return r
  }
  g.__render = async function (url, options) {
    var o = options && typeof options === 'object' ? options : {}
    var r = JSON.parse(await rawRender(String(url), JSON.stringify(o)))
    if (r.error) throw new Error(r.error)
    return r
  }
  g.__solve = async function (url) {
    var r = JSON.parse(await rawSolve(String(url)))
    if (r.error) throw new Error(r.error)
    return r
  }
  delete g.__fetch_raw; delete g.__render_raw; delete g.__solve_raw

  var src = null
  g.registerSource = function (s) { src = s }
  g.__has_source = function () { return !!src }
  g.__methods = function () {
    var out = []; if (!src) return JSON.stringify(out)
    for (var k in src) if (typeof src[k] === 'function') out.push(k)
    return JSON.stringify(out)
  }
  g.__call = async function (m, args) {
    if (!src || typeof src[m] !== 'function') throw new Error('Plugin does not implement ' + m)
    var r = await src[m].apply(src, JSON.parse(args))
    return r === undefined ? 'null' : JSON.stringify(r)
  }
})(globalThis)
