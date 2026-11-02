# Remoport

**Your remote computers and apps, from Linux.**

Remoport is a desktop client for reaching Windows PCs, remote desktops and the apps
your organization publishes, with a clean connection center: favorites, PCs and apps
side by side, and workspaces that bring in what your organization offers.

> **Status: early preview.** Remote desktops open inside Remoport through the RDP
> engine of [RustConn](https://github.com/totoshko88/RustConn) by Anton Isaiev
> (IronRDP-based, GPL-3.0-or-later). This first phase covers the screen, keyboard and
> mouse, and a desktop that follows the window's size; it still awaits testing against
> real servers. Clipboard, sound and shared folders come next; meanwhile any session
> can also be opened in FreeRDP.

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

> **Estado: vista previa.** Los escritorios remotos se abren dentro de Remoport con el
> motor RDP de [RustConn](https://github.com/totoshko88/RustConn), de Anton Isaiev.
> Esta primera fase cubre pantalla, teclado y ratón, y un escritorio que se ajusta al
> tamaño de la ventana; falta probarla contra servidores reales. El portapapeles, el
> sonido y las carpetas compartidas vienen después; mientras tanto, cualquier sesión
> también se puede abrir en FreeRDP.

Remoport es software libre bajo la **GNU GPL, versión 3 o posterior**. El nombre y el
logo de Remoport no están cubiertos por esa licencia: consulta
[`TRADEMARKS.md`](TRADEMARKS.md). Remoport es un proyecto independiente, sin relación
con Microsoft.
