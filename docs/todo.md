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

suggestions to improve MdEditor