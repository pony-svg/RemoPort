# Remoport

**Your remote computers and apps, from Linux.**

Remoport is a desktop client for reaching Windows PCs, remote desktops and the apps
your organization publishes, with a clean connection center: favorites, PCs and apps
side by side, and workspaces that bring in what your organization offers.

> **Status: early preview.** The connection center works; remote sessions do not
> yet. Remoport will connect through the RDP engine of
> [RustConn](https://github.com/totoshko88/RustConn) (IronRDP-based, GPL-3.0-or-later)
> instead of writing one from scratch. Until then, a session can be opened in FreeRDP.

## Build and run

```bash
cargo build --release
./target/release/remoport
```

## License

Remoport is free software under the **GNU General Public License, version 3 or later**
(see [`LICENSE`](LICENSE)). The Remoport name and logo are not covered by that license;
see [`TRADEMARKS.md`](TRADEMARKS.md).

Remoport is an independent project. It is not affiliated with, endorsed by or
sponsored by Microsoft. Windows is a trademark of the Microsoft group of companies.

---

## En español

**Tus equipos y apps remotas, desde Linux.**

Remoport es un cliente de escritorio para conectarte a PCs con Windows, escritorios
remotos y las apps que publica tu organización, con un centro de conexiones claro:
favoritos, PCs y apps juntos, y espacios de trabajo que traen lo que tu organización
te ofrece.

> **Estado: vista previa.** El centro de conexiones funciona; las sesiones remotas
> todavía no. Remoport se conectará con el motor RDP de
> [RustConn](https://github.com/totoshko88/RustConn) en lugar de escribir uno desde
> cero. Mientras tanto, una sesión se puede abrir en FreeRDP.

Remoport es software libre bajo la **GNU GPL, versión 3 o posterior**. El nombre y el
logo de Remoport no están cubiertos por esa licencia: consulta
[`TRADEMARKS.md`](TRADEMARKS.md). Remoport es un proyecto independiente, sin relación
con Microsoft.
