App 用到的几个轻量大模型。

# Parakeet v3
Nvidia 开源的 STT 语音识别模型。
有人制作成 onnx 格式。
借助 Rust 的 pykeio/ort 库，支持 cpu，android 手机也可。
支持多种欧洲语言。
支持生成每个单词的时间点，
因此支持字幕制作，语速计算。

# Whisper
暂不支持生成每个单词的时间点
尚未测试 android 手机支持
其余可替代 Parakeet

# SaT 3L Small
用于将文本切分成句子。
Python 库 NLTK/SpaCy 的完美替代。
用于处理格式混杂的电子书。
原生 onnx 格式，430M。
由剑桥(英国)和林茨大学(奥地利)的研究人员开源的模型。

# Wav2Vec2
Meta 开源的语音识别模型。
不是 STT，而是精确到单词内的每个发音。
可以和标准音频比对，统计出哪些发音不太准确。
TODO: 测试能否在 android 手机上运行。