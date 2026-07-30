pkgname=bts
pkgver=0.0.0
pkgrel=1
pkgdesc="Bansleben Telephone Services phone-controlled display system"
arch=('x86_64')
url="https://github.com/jedrek0429/bts"
license=('GPL-3.0-or-later')
depends=('asterisk' 'cage' 'seatd' 'fontconfig' 'ttf-impallari-cabin-font')
makedepends=('cargo' 'git')
backup=('etc/bts/bts.env')
options=('!debug')
source=()
sha256sums=()

pkgver() {
  cd "$startdir"

  if git describe --tags --abbrev=7 >/dev/null 2>&1; then
    git describe --tags --abbrev=7 | sed -e 's/^v//' -e 's/-\([0-9]\+\)-g/.r\1.g/'
  else
    printf "0.0.0.r%s.g%s" "$(git rev-list --count HEAD)" "$(git rev-parse --short=7 HEAD)"
  fi
}

build() {
  cd "$startdir"
  cargo build --locked --release --workspace
}

package() {
  cd "$startdir"

  install -Dm755 target/release/bts-core "$pkgdir/usr/bin/bts-core"
  install -Dm755 target/release/bts-addons "$pkgdir/usr/bin/bts-addons"
  install -Dm755 target/release/bts-telephony "$pkgdir/usr/bin/bts-telephony"
  install -Dm755 target/release/bts-display "$pkgdir/usr/bin/bts-display"

  install -Dm644 deploy/systemd/bts-core.service "$pkgdir/usr/lib/systemd/system/bts-core.service"
  install -Dm644 deploy/systemd/bts-addons.service "$pkgdir/usr/lib/systemd/system/bts-addons.service"
  install -Dm644 deploy/systemd/bts-telephony.service "$pkgdir/usr/lib/systemd/system/bts-telephony.service"
  install -Dm644 deploy/systemd/bts-display.service "$pkgdir/usr/lib/systemd/system/bts-display.service"
  install -Dm644 deploy/systemd/bts.target "$pkgdir/usr/lib/systemd/system/bts.target"
  install -Dm644 deploy/pacman/bts.hook "$pkgdir/usr/share/libalpm/hooks/bts.hook"

  install -Dm640 deploy/bts.env.example "$pkgdir/etc/bts/bts.env"
  install -Dm755 scripts/bts-install "$pkgdir/usr/bin/bts-install"
  install -Dm755 scripts/generate-voice-prompts.sh "$pkgdir/usr/lib/bts/generate-voice-prompts"
  install -Dm644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}
