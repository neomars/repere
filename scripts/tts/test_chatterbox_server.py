"""Tests du serveur de voix sans PyTorch ni GPU : faux modèle + mode TTS_FAKE.
Lancer :  python3 -m unittest scripts/tts/test_chatterbox_server.py   (numpy requis pour la partie « cache de voix »)
"""
import json
import os
import sys
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.request
from argparse import Namespace
from http.server import ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import chatterbox_server as cs  # noqa: E402

try:
    import numpy as np
except ImportError:  # pragma: no cover
    np = None


class FakeTensor:
    def __init__(self, a): self.a = a
    def squeeze(self): return self
    def detach(self): return self
    def cpu(self): return self
    def numpy(self): return self.a


class FakeModel:
    """Reproduit le contrat utile de ChatterboxMultilingualTTS.generate : conds + audio_prompt_path."""
    sr = 24000

    def __init__(self):
        self.conds = "DEFAULT"
        self.calls = []
        self.fail_next = False
        self.oom = 0  # nombre de prochains appels qui échouent faute de mémoire GPU

    def generate(self, text, language_id, audio_prompt_path=None, **kw):
        self.calls.append((language_id, audio_prompt_path))
        if self.oom:
            self.oom -= 1
            raise RuntimeError("CUDA out of memory. Tried to allocate 20.00 MiB")
        if audio_prompt_path:
            if self.fail_next:
                self.fail_next = False
                raise ValueError("échantillon illisible")  # prepare_conditionals échoue avant d'assigner conds
            self.conds = "VOICE:" + audio_prompt_path
        if self.conds is None:
            raise AssertionError("Please `prepare_conditionals` first or specify `audio_prompt_path`")
        return FakeTensor(np.zeros(240, dtype="float32"))


def make_backend(tmp, default_conds="DEFAULT"):
    class B(cs.Backend):
        def _load(self):
            self.model = FakeModel()
            self.model.conds = default_conds
            self.default_conds = default_conds
    os.environ.pop("TTS_FAKE", None)
    a = Namespace(voices_dir=str(tmp), exaggeration=0.6, cfg_weight=0.35, temperature=0.8, language="fr", device="cpu", model_dir=str(tmp))
    return B(a)


def put_voice(tmp, name="camille", data=b"RIFF....WAVE"):
    p = Path(tmp) / f"{name}.wav"
    p.write_bytes(data)
    return p


@unittest.skipIf(np is None, "numpy absent")
class VoiceCache(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()

    def synth(self, be, voice="camille"):
        return be.synth("Bonjour.", voice, 0.7, 0.3, 0.8, "fr")

    def test_echantillon_analyse_une_fois_puis_reutilise(self):
        put_voice(self.tmp)
        be = make_backend(self.tmp)
        self.synth(be); self.synth(be)
        prompts = [c[1] for c in be.model.calls]
        self.assertTrue(prompts[0].endswith("camille.wav"))
        self.assertIsNone(prompts[1], "la voix de référence n'est pas ré-analysée à chaque phrase")

    def test_echantillon_remplace_sous_le_meme_nom_est_reanalyse(self):
        p = put_voice(self.tmp, data=b"RIFF....WAVE" + b"a" * 10)
        be = make_backend(self.tmp)
        self.synth(be)
        p.write_bytes(b"RIFF....WAVE" + b"b" * 50)  # nouvel enregistrement, même nom, taille différente
        self.synth(be)
        self.assertTrue(be.model.calls[-1][1].endswith("camille.wav"), "doit repasser audio_prompt_path")

    def test_echec_ne_ressert_jamais_une_autre_voix(self):
        put_voice(self.tmp)
        be = make_backend(self.tmp)
        be.model.fail_next = True
        with self.assertRaises(ValueError):
            self.synth(be)
        self.assertIsNone(be.cur_voice)
        self.assertEqual(be.model.conds, "DEFAULT")
        self.synth(be)  # le 2e essai repasse bien l'échantillon (au lieu de réutiliser silencieusement la voix par défaut)
        self.assertTrue(be.model.calls[-1][1].endswith("camille.wav"))

    def test_memoire_gpu_saturee_un_nouvel_essai_suffit(self):
        be = make_backend(self.tmp)
        be.model.oom = 1
        self.synth(be, voice=None)  # ne lève pas : le 2e essai passe
        self.assertEqual(len(be.model.calls), 2)

    def test_memoire_gpu_saturee_message_utile(self):
        be = make_backend(self.tmp)
        be.model.oom = 2
        with self.assertRaisesRegex(RuntimeError, "mémoire GPU insuffisante"):
            self.synth(be, voice=None)
        self.synth(be, voice=None)  # le serveur reste utilisable ensuite

    def test_retour_a_la_voix_par_defaut(self):
        put_voice(self.tmp)
        be = make_backend(self.tmp)
        self.synth(be)
        self.synth(be, voice="inconnue")
        self.assertEqual(be.model.conds, "DEFAULT")
        self.assertIsNone(be.cur_voice)

    def test_sans_voix_par_defaut_erreur_claire(self):
        be = make_backend(self.tmp, default_conds=None)
        with self.assertRaisesRegex(RuntimeError, "aucune voix par défaut"):
            self.synth(be, voice=None)
        put_voice(self.tmp)
        self.synth(be)                       # une voix de référence rend le serveur utilisable
        with self.assertRaisesRegex(RuntimeError, "aucune voix par défaut"):
            self.synth(be, voice=None)       # et le retour à « défaut » ne ressert pas l'ancienne voix


class Http(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        os.environ["TTS_FAKE"] = "1"
        cls.tmp = tempfile.mkdtemp()
        put_voice(cls.tmp)
        a = Namespace(voices_dir=cls.tmp, exaggeration=0.6, cfg_weight=0.35, temperature=0.8, language="fr", device="cpu", model_dir=cls.tmp)
        cls.be = cs.Backend(a)
        cls.srv = ThreadingHTTPServer(("127.0.0.1", 0), cs.make_handler(cls.be, a))
        cls.port = cls.srv.server_address[1]
        threading.Thread(target=cls.srv.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.srv.shutdown()
        os.environ.pop("TTS_FAKE", None)

    def post(self, body, raw=False):
        data = body if raw else json.dumps(body).encode()
        req = urllib.request.Request(f"http://127.0.0.1:{self.port}/v1/audio/speech", data=data, method="POST")
        try:
            with urllib.request.urlopen(req) as r:
                return r.status, r.read()
        except urllib.error.HTTPError as e:
            return e.code, e.read()

    def test_parole_et_langue_regionale(self):
        for lang in ("fr", "fr-FR", "fr_FR"):
            code, body = self.post({"input": "Bonsoir.", "voice": "camille", "language": lang})
            self.assertEqual(code, 200, lang)
            self.assertEqual(body[:4], b"RIFF")

    def test_entrees_invalides_donnent_400(self):
        self.assertEqual(self.post(b"pas du json", raw=True)[0], 400)
        self.assertEqual(self.post([1, 2, 3])[0], 400)
        self.assertEqual(self.post({"input": "   "})[0], 400)

    def test_liste_des_voix(self):
        with urllib.request.urlopen(f"http://127.0.0.1:{self.port}/v1/voices") as r:
            self.assertEqual(json.loads(r.read())["voices"], ["camille"])


if __name__ == "__main__":
    unittest.main()
