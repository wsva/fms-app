import psycopg
import uuid
import nltk
import re

# Download tokenizer data once
nltk.download("punkt", quiet=True)

DB_CONFIG = {
    "dbname": "fmsdb",
    "user": "postgres",
    "password": "222222",
    "host": "1111",
    "port": "15432",
}

TEXT_FILE = "data/verity_en.txt"

def split_sentences(text):
    # Split on one or more blank lines
    # paragraphs = re.split(r'\n\s*\n+', text.strip())
    paragraphs = re.split(r'\s*\n\s*', text.strip())

    sentences = []
    for paragraph in paragraphs:
        paragraph = paragraph.strip()
        if not paragraph:
            continue

        sentences.extend(nltk.sent_tokenize(paragraph))

    return sentences

def insert_sentences(conn, chunk_uuid, sentences):
    sql = """
        INSERT INTO listen_subtitle_reference (uuid, chunk_uuid, order_num, content)
        VALUES (%s, %s, %s, %s)
    """

    data = []

    for order_num, sentence in enumerate(sentences, start=1):
        sentence = sentence.strip()

        if not sentence:
            continue

        data.append(
            (
                str(uuid.uuid4()).replace("-", ""),
                chunk_uuid,
                order_num,
                sentence
            )
        )

    with conn.cursor() as cursor:
        cursor.executemany(sql, data)

    conn.commit()


def main():
    chunk_uuid = str(uuid.uuid4()).replace("-", "")

    # Read text file
    with open(TEXT_FILE, "r", encoding="utf-8") as f:
        text = f.read()

    # Split into sentences
    sentences = split_sentences(text)

    print(f"Chunk UUID: {chunk_uuid}")
    print(f"Found {len(sentences)} sentences")

    # Connect to PostgreSQL
    conn = psycopg.connect(**DB_CONFIG)

    try:
        insert_sentences(conn, chunk_uuid, sentences)
        print("Sentences inserted successfully")
    finally:
        conn.close()


if __name__ == "__main__":
    main()