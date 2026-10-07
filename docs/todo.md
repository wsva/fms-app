read a book


OCR page
ocrs
https://github.com/robertknight/ocrs
slect camera

automate dataset making
1, a status file, for the status of every aspects of a dataset
2, use agent to complete every job according to status

local chat
1, send text, image or all file formats between android and pc

secret dataset
1, all data encrypted
2, used to store all passwords


support two pc
1, device chat
2, datasets sync
3, datasets sync between two pc is complex, both save complete data
4, only one pc is the fact of truth, other pcs are like android
5, add role, when pairing, if both pcs are main node role, will fail
6, don't start web service on non-main node
7, or we can implement pc-pc sync to support main-main nodes?

patch mechanism of dataset
1, a new dir dataset/patches
2, new table, save latest patch version
3, use this to update to latest version by applying patch one by one
4, problem: when to generate a patch?