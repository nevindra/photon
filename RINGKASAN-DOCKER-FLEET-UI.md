# Ringkasan Pekerjaan: UI Docker Fleet Management untuk Photon

Branch: `feat/docker-fleet-ui` (belum di-commit). Semua pekerjaan **frontend-only**, berjalan di atas
korpus data tiruan (mock) di browser karena backend untuk subsistem ini belum ada.

## 1. Titik awal

Permintaan awal: membaca dokumen kebutuhan `docker-fleet-management.md` (katalog template, push-deploy
ke site on-prem milik pelanggan lewat agent yang melakukan polling keluar, dan keterkaitan dengan
telemetri), memecahnya menjadi beberapa fase, lalu **menguji setiap fase** setelah selesai (Playwright,
environment, dsb). Batasan yang diminta: fokus sepenuhnya pada **UI/UX** — mudah dipakai, elegan,
indah, dan konsisten dengan design system serta warna brand Photon.

Konvensi proyek yang dipatuhi sepanjang pengerjaan:

- Vue 3 + Vite + Tailwind + Reka UI, ikon Lucide, TanStack Query (tanpa Pinia), klien Ky dengan
  fallback ke mock.
- Package manager **bun** (bukan npm). File baru ditulis dalam TypeScript (`<script setup lang="ts">`).
- Hanya memakai token desain (`--brand` cyan, `sev-*`, `success`, ramp elevasi datar, keluarga radius).
- Dokumentasi harus selalu sinkron dengan kode (`docs/subsystems/deploy.md`, `docs/frontend.md`,
  `CLAUDE.md`, `CONTEXT.md`).
- Tidak pernah commit/push tanpa diminta.

## 2. Pembagian fase dan apa yang dibangun

### Fase 0 — Fondasi: peran, audit, agent, enrolment host
- Tipe dan ~45 metode API baru di `lib/core/api.ts` untuk `/api/agents*`, `/api/audit`, `/api/stacks*`,
  `/api/templates*`, `/api/registries*`, `/api/rollouts*`, `/api/exec/*`.
- Peran `viewer` / `operator` / `admin` (`lib/deploy/roles.ts`), fungsi `can()` di `lib/core/auth.ts`,
  dan komponen `GatedButton` yang **menonaktifkan** tombol (bukan menyembunyikan) sambil menjelaskan
  alasannya lewat tooltip.
- Halaman `/audit` (`AuditView`) — log setiap perubahan yang Photon lakukan pada host.
- Dialog **Add host** di `/infra`: bootstrap lewat SSH (stepper yang berjalan menurut waktu) atau token
  yang dicetak server, pemilih kapabilitas agent (`read` / `lifecycle` / `apply` / `exec`).
- Tab **Agent** di `/infra/:host/agent`, strip perhatian untuk agent offline / menunggu sinkron pertama.

### Fase 1 — Tab Containers (read-only)
- `/infra/:host/containers`: KPI (berjalan/total, unhealthy, restart, disk Docker), container
  dikelompokkan per proyek compose, tabel images / volumes / networks, feed event daemon.
- Drawer container dengan tab **Logs** (follow, tautan trace-id), **Stats**, **Details** (env dengan
  nilai rahasia tersamar), dan **Shell** (diaudit, hanya admin + agent dengan `--docker-exec`).

### Fase 2 — Aksi siklus hidup
- Aksi satu-tembakan: start / stop / restart / kill / pull / recreate / remove / prune, dengan dialog
  konfirmasi yang **disesuaikan dengan radius ledakannya** (klik biasa, konfirmasi, atau ketik nama).
- Semantik **queued-when-offline**: aksi untuk host offline menunggu sebagai pending, kedaluwarsa
  15 menit.
- Strip "Recent actions" yang berdetak, toast hanya untuk aksi baru (bukan riwayat saat pertama buka).

### Fase 3 — Stacks
- `/deploy/stacks` + detail: proyek compose di bawah *desired state*, satu per host.
- Editor compose dengan lint saat mengetik; konstruk berbahaya (privileged, bind mount di luar prefix
  yang diizinkan, dll.) wajib **diakui secara eksplisit** sebelum dikirim.
- Opsi deploy (pull policy, force recreate, recreate volumes), konfirmasi bertingkat untuk
  apply / restart / redeploy / down / destroy.
- **Drift** ditampilkan *desired* di samping *observed*; reconcile dan accept adalah tindakan sadar.
- **Adopt existing**: mengambil alih proyek compose yang sudah berjalan tanpa membuat ulang container.
- Tautan ke layanan APM yang dihasilkan stack.

### Fase 4 — Templates
- `/deploy/templates` + detail: katalog resep berversi dan berparameter (variabel bertipe: string,
  port, path, select, bool, secret, generated, preset), versi immutable, registry, impor JSON Portainer.
- **Wizard deploy empat langkah**: target → variabel bertipe → preflight terhadap fakta host
  (kapabilitas, online, arsitektur, Docker, RAM, disk, port, image) dengan **diff compose** dan
  pengakuan konstruk berbahaya → deploy. Juga jalur **upgrade** untuk stack yang lahir dari template.
- Publish versi baru, matriks versi, diff antarversi.

*(Sesuai permintaan, pekerjaan berhenti di sini untuk ringkasan, lalu dilanjutkan.)*

### Fase 5 — Rollouts
- `/deploy/rollouts` + detail: satu versi template ke banyak host, **bertahap**: canary → gate
  eksplisit → sisanya. Target dipilih lewat host, Site (tenant federasi), atau kecocokan label.
- Status per target: `queued`, `pending` (host tidak menjawab — netral, tidak pernah merah),
  `prepulling`, `applying`, `healthy`, `failed`, `drifted`, `skipped`.
- Ambang kegagalan yang **menjeda** tahap; rollback sebagai apply ulang versi immutable sebelumnya.
- Panel **Canary verdict** di gate: promosikan berdasarkan bukti, bukan hanya apply yang hijau.
- Supervisor mock berbasis waktu (`tick()`) agar alur bisa didemokan.

### Fase 6 — Integrasi dan polesan
- **Attention Board** (`/home`) mendapat dunia **Deploy**: Stack dan Rollout menjadi Subject dengan
  Attention Level; agent offline = Down, disk ≥ 90 % = Critical.
- Panel **"Did it work?"** (`DeployImpactPanel`): layanan APM stack dinilai 15 menit sebelum vs
  sesudah apply.
- Tab Shell yang diaudit, sapuan mode gelap, navigasi rail dengan dunia Deploy
  (Templates / Stacks / Rollouts) dan Audit di bawah Manage.

## 3. Sakelar ketersediaan (agar instalasi nyata tidak melihat data palsu)

`lib/deploy/availability.ts`: backend tanpa subsistem menjawab rute deploy dengan **404 kosong**.
Hanya dalam **mode demo** (build dev, `localStorage['photon-deploy-demo']='1'`, atau tanpa backend
sama sekali) korpus mock (`lib/core/mockDeploy.ts`, dimuat malas) menggantikannya. Selain itu setiap
pembacaan melempar `DeployUnavailableError` dan setiap layar Deploy menampilkan panel "belum
diaktifkan". 404 berbadan JSON selalu dianggap jawaban nyata. Peran mengikuti aturan yang sama:
kontrol peran hanya muncul bila sesi membawa `role`.

## 4. Siklus review independen dan iterasi

Sesuai permintaan, sebuah **agent reviewer terpisah tanpa konteks** menjalankan aplikasi sendiri
dan memberi skor jujur dari 100. Ditambah `code-review` yang menemukan 10 isu ketepatan
(semuanya diperbaiki).

| Putaran | Skor | Isi |
|---|---|---|
| 1 | 71 | Jalan penuh enam fase, 20 temuan |
| 2 | 82 | Semua 20 diverifikasi terpecahkan, 10 masih terbuka |
| 3 | 85 | 9 dari 10 diverifikasi, 1 tak bisa direproduksi, 5 temuan baru |

Perbaikan utama sepanjang iterasi:

- Sakelar ketersediaan di atas, plus **uji unit** `lib/core/api.deploy.test.ts` yang membuktikannya
  di luar build dev (menstub `import.meta.env.DEV` dan fetch yang menjawab 404 kosong; juga menstub
  `Request` karena ky menolak URL relatif di jsdom sebelum fetch sempat berjalan).
- Rollout: **perencanaan dilakukan setelah preflight** — host yang gagal dry-run tak pernah jadi
  canary; ringkasan Review hanya menghitung host yang direncanakan, menyebut setiap host yang dilewati
  beserta alasannya, dan memblokir pembuatan bila tak ada host lolos. Secret yang sudah dimiliki semua
  target terbaca "Stored — leave blank to keep".
- **Bug kritis temuan reviewer**: nilai pin versi lama yang tersimpan di stack mengalahkan preset versi
  baru saat submit (form menampilkan 2.4.2, yang terkirim 2.4.1). Kini preset **selalu** memakai
  default template; diff upgrade sekarang memindahkan kedua tag image. Diuji unit.
- Diff upgrade tak lagi menampilkan perubahan palsu pada blok observability Photon
  (`OTEL_EXPORTER_OTLP_ENDPOINT` dirender konsisten di kedua sisi).
- `/infra`: host terkelola tanpa metrik sumber daya mendapat kartu sendiri (`ManagedHostCard`)
  alih-alih layar "No hosts reporting" yang kontradiktif.
- Tab Images: aksi **pull / remove per image** (remove hanya untuk image tak terpakai, tetap
  dikonfirmasi, diaudit). Kolom Actions muat di lebar 1024 px.
- Adopt hanya butuh kapabilitas `read`; container hasil adopsi langsung tampil sebagai layanan yang
  berjalan; tombol yang dinonaktifkan karena kapabilitas menjelaskan alasannya saat hover.
- Tautan log stack diberi cakupan `compose.project`, bukan seluruh host.
- Panel dampak tanpa trafik dilipat menjadi satu kalimat dengan jendela waktu absolut.
- Berbagai perbaikan salinan teks (pluralisasi, "0 hosts in 1 stage", hint host di mode adopt).

## 5. Pengujian

- Setiap fase: vitest + `bun run type-check` + tangkapan layar Playwright terhadap `make dev`
  (Vite :5173, backend :8080 yang 404 pada rute baru sehingga UI jatuh ke mode demo).
- Status akhir: **196 file uji, 1451 tes lulus; vue-tsc bersih**; seluruh perubahan putaran terakhir
  dicek langsung di browser (tab Images, `/infra`, diff upgrade, wizard rollout, adopt, tata letak
  1024 px).

## 6. Yang belum dikerjakan

- **Seluruh backend** subsistem ini (crate `photon-deploy`, rute `/api/agents|stacks|templates|rollouts|audit`).
- Item opsional (MAY) dalam spesifikasi: stack berbasis git, bundel offline, persetujuan dua orang.
- Catatan kecil dari reviewer yang sengaja tidak dikejar: overflow 22 px pada kolom chevron di
  1024 px, dan 5–8 request 404 per tampilan saat backend belum punya subsistem ini.

Semua pekerjaan masih **belum di-commit** di branch `feat/docker-fleet-ui`.
