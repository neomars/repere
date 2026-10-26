#!/usr/bin/env python3
"""Serveur de synthèse vocale compatible OpenAI (/v1/audio/speech) pour Chatterbox multilingue.

Lancé et arrêté par live-engine. Le modèle est chargé AVANT l'ouverture du port : dès que le port répond,
la voix est prête (même convention que whisper-server).

  POST /v1/audio/speech  {"input": "...", "voice": "camille", "exaggeration": 0.7, "cfg_weight": 0.3,
                          "temperature": 0.8, "language": "fr"}      → audio/wav (24 kHz, mono)
  GET  /v1/voices        → {"voices": ["camille", ...]}   (fichiers de référence de --voices-dir)
  GET  /health           → {"status": "ok"}

La voix vient d'un court échantillon (≈ 10-20 s, une seule personne, sans bruit ni musique) placé dans
--voices-dir sous le nom <voix>.wav / .mp3 / .flac / .ogg. Sans échantillon : voix par défaut du modèle.
TTS_FAKE=1 : génère un simple bip (tests sans GPU ni modèle).
"""
import argparse
import array
import gc
import io
import json
import math
import os
import re
import sys
import threading
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

AUDIO_EXT = (".wav", ".mp3", ".flac", ".ogg")
# Fichiers exigés par ChatterboxMultilingualTTS.from_local (conds.pt est facultatif).
REQUIRED_FILES = ("ve.pt", "t3_mtl23ls_v2.safetensors", "s3gen.pt", "grapheme_mtl_merged_expanded_v1.json")
NAME_RE = re.compile(r"^[A-Za-z0-9_-]{1,64}$")
MAX_CHARS = 1200


def log(*a):
    print(*a, file=sys.stderr, flush=True)


def is_oom(e):
    """Mémoire GPU saturée (torch.cuda.OutOfMemoryError, ou RuntimeError « out of memory » selon la version)."""
    return type(e).__name__ == "OutOfMemoryError" or "out of memory" in str(e).lower()


def free_cuda_cache():
    gc.collect()
    try:
        import torch

        if torch.cuda.is_available():
            torch.cuda.empty_cache()
    except Exception:  # noqa: BLE001 — sans torch (tests) ou sans CUDA : rien à vider
        pass


class Backend:
    def __init__(self, a):
        self.a = a
        self.fake = os.environ.get("TTS_FAKE") == "1"
        self.lock = threading.Lock()
        self.cur_voice = None
        self.sr = 24000
        self.model = None
        self.default_conds = None
        if not self.fake:
            self._load()

    def _load(self):
        import torch  # noqa: import tardif : lent, et inutile en mode TTS_FAKE

        # Chatterbox applique un filigrane audio inaudible (perth). perth attrape TOUT ImportError (pkg_resources absent,
        # numpy/librosa cassé…) et met alors son filigrane à None : la bibliothèque plante plus loin avec un
        # « NoneType is not callable » incompréhensible. On importe donc le vrai module pour montrer la vraie cause.
        try:
            import perth
            from perth.perth_net.perth_net_implicit.perth_watermarker import PerthImplicitWatermarker  # noqa: F401

            if getattr(perth, "PerthImplicitWatermarker", None) is None:
                raise ImportError("perth.PerthImplicitWatermarker vaut None")
        except ImportError as e:
            raise SystemExit(
                f"✗ le filigrane audio « perth » ne s'importe pas ({e}). Relance « Mettre à jour le moteur de voix » "
                "(il installe setuptools<81 et vérifie perth), ou supprime ~/.local/share/repere/tts-venv/.ready "
                "et réinstalle."
            )
        from chatterbox.mtl_tts import ChatterboxMultilingualTTS

        device = self.a.device
        if device == "cuda" and not torch.cuda.is_available():
            log("! CUDA indisponible : repli sur le CPU (très lent)")
            device = "cpu"
        model_dir = Path(self.a.model_dir)
        log(f"chargement de Chatterbox multilingue ({device}) depuis {model_dir} …")
        missing = [f for f in REQUIRED_FILES if not (model_dir / f).is_file()]
        if not missing:
            # Toute erreur de from_local est remontée telle quelle : elle n'a rien à voir avec des fichiers absents.
            self.model = ChatterboxMultilingualTTS.from_local(str(model_dir), device)
        else:
            log(f"! fichiers absents de {model_dir} ({', '.join(missing)}) : téléchargement par la bibliothèque (plusieurs Go)")
            self.model = ChatterboxMultilingualTTS.from_pretrained(device=device)
        self.sr = getattr(self.model, "sr", 24000)
        self.default_conds = getattr(self.model, "conds", None)
        if self.default_conds is None and not self.voices():
            log("! ni conds.pt ni voix de référence : ajoute un échantillon (Admin → Profils d'assistant → Voix) pour parler")
        log("modèle chargé")

    def voices(self):
        d = Path(self.a.voices_dir)
        if not d.is_dir():
            return []
        return sorted({p.stem for p in d.iterdir() if p.suffix.lower() in AUDIO_EXT and NAME_RE.match(p.stem)})

    def voice_path(self, name):
        if not name or not NAME_RE.match(name):
            return None
        for ext in AUDIO_EXT:
            p = Path(self.a.voices_dir) / (name + ext)
            if p.is_file():
                return str(p)
        return None

    def _generate(self, text, language, kw):
        """generate() ; si la mémoire GPU est saturée (LLM + micro + voix sur une même carte), on vide le cache CUDA et
        on réessaie une fois avant de renvoyer une erreur qui dit quoi faire."""
        try:
            return self.model.generate(text, language_id=language, **kw)
        except Exception as e:  # noqa: BLE001
            if not is_oom(e):
                raise
            log("! mémoire GPU saturée : cache CUDA vidé, nouvel essai")
            free_cuda_cache()
            try:
                return self.model.generate(text, language_id=language, **kw)
            except Exception as e2:  # noqa: BLE001
                if is_oom(e2):
                    raise RuntimeError(
                        "mémoire GPU insuffisante pour la voix : choisissez une IA plus petite (Admin → Modèles IA) "
                        "ou fermez les programmes qui utilisent la carte, puis rechargez la voix"
                    ) from e2
                raise

    def synth(self, text, voice, exaggeration, cfg_weight, temperature, language):
        """Retourne (échantillons int16 sous forme de array('h'), fréquence)."""
        if self.fake:
            hz = 180 + (sum(map(ord, voice or "")) % 120)
            n = int(self.sr * min(0.6, 0.05 + len(text) / 120))
            return array.array("h", (int(9000 * math.sin(2 * math.pi * hz * i / self.sr)) for i in range(n))), self.sr
        import numpy as np

        path = self.voice_path(voice)
        key = None
        if path is not None:
            try:  # clé de cache : un échantillon remplacé sous le même nom doit être ré-analysé
                st = os.stat(path)
                key = (path, st.st_mtime_ns, st.st_size)
            except OSError:
                path = None
        with self.lock:
            kw = dict(exaggeration=exaggeration, cfg_weight=cfg_weight, temperature=temperature)
            need_prompt = key is not None and key != self.cur_voice
            if need_prompt:
                kw["audio_prompt_path"] = path  # la voix de référence n'est analysée que si elle change
            elif key is None:
                # Voix par défaut du modèle (conds.pt) : la restaurer si une autre voix était active.
                if self.cur_voice is not None:
                    if self.default_conds is None:
                        raise RuntimeError("aucune voix par défaut dans le modèle (conds.pt absent) : choisis une voix de référence")
                    self.model.conds = self.default_conds
                    self.cur_voice = None
                elif getattr(self.model, "conds", None) is None:
                    raise RuntimeError("aucune voix par défaut dans le modèle (conds.pt absent) : choisis une voix de référence")
            try:
                wav = self._generate(text, language, kw)
            except Exception:
                # État incertain (échantillon illisible, trop court…) : jamais resservir une autre voix par erreur.
                self.model.conds = self.default_conds
                self.cur_voice = None
                raise
            if need_prompt:
                self.cur_voice = key
        pcm = (np.clip(wav.squeeze().detach().cpu().numpy(), -1.0, 1.0) * 32767).astype("int16")
        return array.array("h", pcm.tolist()), self.sr


def to_wav(samples, sr):
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sr)
        w.writeframes(samples.tobytes())
    return buf.getvalue()


def make_handler(be, a):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *x):
            pass

        def _json(self, code, obj):
            b = json.dumps(obj).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)

        def do_GET(self):
            if self.path.startswith("/v1/voices"):
                return self._json(200, {"voices": be.voices()})
            self._json(200, {"status": "ok", "fake": be.fake})

        def do_POST(self):
            if not self.path.startswith("/v1/audio/speech"):
                return self._json(404, {"error": "route inconnue"})
            try:
                n = int(self.headers.get("Content-Length", 0))
                req = json.loads(self.rfile.read(n) or b"{}")
            except (ValueError, UnicodeDecodeError):
                return self._json(400, {"error": "corps JSON invalide"})
            if not isinstance(req, dict):
                return self._json(400, {"error": "le corps doit être un objet JSON"})
            try:
                text = str(req.get("input", "")).strip()[:MAX_CHARS]
                if not text:
                    return self._json(400, {"error": "champ « input » vide"})
                num = lambda k, d: float(req[k]) if req.get(k) is not None else d  # noqa: E731
                samples, sr = be.synth(
                    text,
                    req.get("voice"),
                    min(2.0, max(0.25, num("exaggeration", a.exaggeration))),
                    min(1.0, max(0.0, num("cfg_weight", a.cfg_weight))),
                    min(1.5, max(0.1, num("temperature", a.temperature))),
                    str(req.get("language") or a.language).replace("_", "-").split("-")[0].lower(),  # « fr-FR » → « fr »
                )
                body = to_wav(samples, sr)
            except Exception as e:  # noqa: BLE001 — on renvoie l'erreur au moteur, qui la journalise
                log("erreur de synthèse :", repr(e))
                return self._json(500, {"error": str(e)})
            self.send_response(200)
            self.send_header("Content-Type", "audio/wav")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    return H


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=8880)
    p.add_argument("--model-dir", default=".")
    p.add_argument("--voices-dir", default="voices")
    p.add_argument("--device", default="cuda")
    p.add_argument("--language", default="fr")
    p.add_argument("--exaggeration", type=float, default=0.6)
    p.add_argument("--cfg-weight", type=float, default=0.35)
    p.add_argument("--temperature", type=float, default=0.8)
    a = p.parse_args()
    Path(a.voices_dir).mkdir(parents=True, exist_ok=True)
    be = Backend(a)
    srv = ThreadingHTTPServer((a.host, a.port), make_handler(be, a))
    log(f"serveur de voix prêt sur http://{a.host}:{a.port} (fake={be.fake}, voix : {be.voices() or 'défaut'})")
    srv.serve_forever()


if __name__ == "__main__":
    main()
