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
1, don't switch paragraph when scrolling

add a STT page:
1, two modes: live mode and batch mode
2, speak and show content

on read-aloud page, on manage page, after upload a audio, cannot use whisper, it says whisper doesn't support word-timing

on card dataset page, after click add card button, and after save and close, return to a new add card page.

on dataset card, on edit view, add a new button "join family", then select another dataset. after select, change the uuid of this dataset. add a warning to the user. So the user can do this manually.

in context menu, remove the "ask the model" section

in context menu, when add card, set the dataset to family card dataset