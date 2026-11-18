"""A small BERT-shaped stand-in embedding model for tests/differential.py (no sentence-embedding model is on the computer
and none is downloaded): a WordPiece tokenizer and an ONNX graph with the three int64 inputs a BERT export has, its token
vectors through MatMul, Tanh and LayerNormalization. Needs onnx, numpy and tokenizers."""
import sys, numpy as np, onnx
from onnx import helper, TensorProto, numpy_helper
from tokenizers import Tokenizer, models, normalizers, pre_tokenizers, processors, decoders
out = sys.argv[1]
words = ("the a an of to and in is it for on with as by at from this that be are was not or network scan scanning port packet "
         "capture sniff wireless wifi password crack hash forensic malware rootkit debug disassembl decompil fuzz reverse "
         "engineer binar encrypt decrypt crypt encod stegan software develop program compil build version control source "
         "code edit web brows download file transfer email chat internet image photo draw vector graphic picture play "
         "record audio music video word process spreadsheet present document note scien math statist data analys plot "
         "config desktop appear input device system prefer admin manag service disk monitor machine util archiv clipboard "
         "text tool sarina search memory session task outcome hello world fire tv remote garage door car key subghz "
         "nfc rfid infrared frequency protocol preset uid type device").split()
vocab = {"[PAD]": 0, "[UNK]": 1, "[CLS]": 2, "[SEP]": 3}
for w in words:
    vocab.setdefault(w, len(vocab))
for w in words:
    for suf in ("s", "ing", "ed", "er", "ion", "ions", "e", "y", "ic", "ics", "ly"):
        vocab.setdefault("##" + suf, len(vocab))
for c in "abcdefghijklmnopqrstuvwxyz0123456789.,;:!?-_=()/'\"":
    vocab.setdefault(c, len(vocab)); vocab.setdefault("##" + c, len(vocab))
tok = Tokenizer(models.WordPiece(vocab=vocab, unk_token="[UNK]", max_input_chars_per_word=100))
tok.normalizer = normalizers.BertNormalizer(lowercase=True)
tok.pre_tokenizer = pre_tokenizers.BertPreTokenizer()
tok.post_processor = processors.TemplateProcessing(single="[CLS] $A [SEP]", pair="[CLS] $A [SEP] $B:1 [SEP]:1",
                                                   special_tokens=[("[CLS]", 2), ("[SEP]", 3)])
tok.decoder = decoders.WordPiece()
tok.save(out + "/tokenizer.json")
V, H, D = len(vocab), 48, 32
rng = np.random.default_rng(7)
init = [numpy_helper.from_array(rng.standard_normal((V, H)).astype(np.float32), "word"),
        numpy_helper.from_array(rng.standard_normal((2, H)).astype(np.float32) * 0.1, "typ"),
        numpy_helper.from_array((rng.standard_normal((H, H)) / np.sqrt(H)).astype(np.float32), "w1"),
        numpy_helper.from_array(rng.standard_normal((H,)).astype(np.float32) * 0.1, "b1"),
        numpy_helper.from_array(np.ones((H,), np.float32), "g"), numpy_helper.from_array(np.zeros((H,), np.float32), "beta"),
        numpy_helper.from_array((rng.standard_normal((H, D)) / np.sqrt(H)).astype(np.float32), "w2"),
        numpy_helper.from_array(np.array([2], np.int64), "ax")]
nodes = [helper.make_node("Gather", ["word", "input_ids"], ["we"]), helper.make_node("Gather", ["typ", "token_type_ids"], ["te"]),
         helper.make_node("Add", ["we", "te"], ["e"]), helper.make_node("MatMul", ["e", "w1"], ["h0"]),
         helper.make_node("Add", ["h0", "b1"], ["h1"]), helper.make_node("Tanh", ["h1"], ["h2"]),
         helper.make_node("LayerNormalization", ["h2", "g", "beta"], ["h3"], axis=-1, epsilon=1e-5),
         helper.make_node("MatMul", ["h3", "w2"], ["h4"]), helper.make_node("Cast", ["attention_mask"], ["mf"], to=TensorProto.FLOAT),
         helper.make_node("Unsqueeze", ["mf", "ax"], ["m3"]), helper.make_node("Mul", ["h4", "m3"], ["last_hidden_state"])]
ins = [helper.make_tensor_value_info(n, TensorProto.INT64, ["batch", "seq"]) for n in ("input_ids", "attention_mask", "token_type_ids")]
outs = [helper.make_tensor_value_info("last_hidden_state", TensorProto.FLOAT, ["batch", "seq", D])]
g = helper.make_graph(nodes, "bert_standin", ins, outs, init)
m = helper.make_model(g, opset_imports=[helper.make_opsetid("", 17)]); m.ir_version = 8
onnx.checker.check_model(m); onnx.save(m, out + "/model.onnx")
print(V, "tokens")
