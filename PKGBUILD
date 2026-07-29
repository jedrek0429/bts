pkgname=bts-git
pkgver=0.1.0.r0
pkgrel=2
pkgdesc="Bansleben Telephone Services phone-controlled display system"
arch=('x86_64')
url="https://github.com/jedrek0429/bts"
license=('MIT')
depends=('asterisk' 'cage' 'seatd' 'fontconfig' 'ttf-impallari-cabin-font')
makedepends=('cargo' 'git')
provides=('bts')
conflicts=('bts')
backup=('etc/bts/bts.env')
options=('!debug')
source=("bts::git+file://$startdir")
sha256sums=('SKIP')

pkgver() {
  cd bts
  printf "0.1.0.r%s.%s" "$(git rev-list --count HEAD)" "$(git rev-parse --short=7 HEAD)"
}

build() {
  cd bts
  cargo build --locked --release --workspace
}

check() {
  cd bts
  cargo test --locked --workspace
}

package() {
  cd bts

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
}
