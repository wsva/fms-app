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
1, call model to generate prompt
2, call agent to send prompt


multi layer of dataset
1, share a dataset
2, download and set to read only
3, all changes write to a new layer
4, original update
5, update the read-only version safely
6, don't affect local changes
7, when read, combine two layers
8, like docker image
9, can merge multi layer into one
10, how to mark a dataset to readonly?
11, this is a preparation for online sharing

make mcp as a feature
can build release without mcp server


status of all loaded models

modify website

add a nornalized field to read aloud
use workflow to normalize text
convert number to word
convert abbreviation to full form

use the recognized text of standard audio as scoring reference


read-aloud, record > saving > saved
reperate from transcribing anf scoring


add a buuton in toolbar, to practice speaking at anytime
open a bottom window

on books page:
1, remove sentence drawer on PC, keep it on android
2, make select a sentence, a paragraph and a chapter more convenient
3, on PC, remove add sentence button, use paragraph as an edit unit
4, books page is not designed for any book, it's only for philosophy books. Difficult, need to read sentence by sentence, need lots of help from AI. For easy books, you should use an E-reader.
5, use left-right layout by default
6, use the right part to edit content
7, when click sentence, in right part: edit, words (link to cards), color, grammer tree, simplification, explain ...
8, when click paragraph, in right part: edit, words (collect from sentences), explain ...
9, when select chapter, in right part: edit, words (collect from sentences), explain ...
10, when select book, in right part: maintain chapters, matain dataset family
11, design a sentence editor:
label: uuid, line num
content
copy content: only copy content
copy all: copy content with uuid
as table, three columns?

add a STT page:
1, two modes: live mode and batch mode
2, speak and show content