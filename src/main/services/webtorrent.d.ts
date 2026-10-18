declare module 'webtorrent' {
  class WebTorrent {
    constructor(opts?: Record<string, unknown>)
    add(torrentId: string, opts?: Record<string, unknown>, cb?: (torrent: Torrent) => void): Torrent
    remove(torrentId: string, opts?: Record<string, unknown>, cb?: (err?: Error) => void): void
    destroy(cb?: (err?: Error) => void): void
    torrents: Torrent[]
  }

  interface Torrent {
    infoHash: string
    magnetURI: string
    files: TorrentFile[]
    downloadSpeed: number
    uploadSpeed: number
    numPeers: number
    progress: number
    ready: boolean
    done: boolean
    length: number
    downloaded: number
    uploaded: number
    on(event: string, cb: (...args: unknown[]) => void): this
    off(event: string, cb: (...args: unknown[]) => void): this
    destroy(cb?: (err?: Error) => void): void
    pause(): void
    resume(): void
  }

  interface TorrentFile {
    name: string
    path: string
    length: number
    streamTo(stream: NodeJS.WritableStream): void
    createReadStream(opts?: { start?: number; end?: number }): NodeJS.ReadableStream
    serve(server: import('http').Server, path: string): string
  }

  export = WebTorrent
}
