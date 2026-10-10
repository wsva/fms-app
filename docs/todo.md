automate dataset making
1, a status file, for the status of every aspects of a dataset
2, use agent to complete every job according to status

secret dataset
1, all data encrypted
2, used to store all passwords



move model info into global db
we can update the models through website
don't need to re-install app


workflow
1, gather infos
2, define steps
3, send to local model
4, generate next jobs


multi layer of dataset
1, share a dataset
2, download and set to read only
3, all changes write to a new layer
4, original update
5, update the read-only version safely
6, don't affect local changes
7, when read, combine two layers
8, like docker image


use sat-3l-sm huggingface
to replace nltk/spacy
then the whole workflow of dictation dataset free of python
use ort to run onnx model
https://github.com/19h/wtsplit-rs
https://huggingface.co/segment-any-text/sat-3l-sm/tree/main


make mcp as a feature
can build release without mcp server