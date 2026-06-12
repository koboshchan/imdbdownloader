import os
import re
import subprocess
import argparse

def main():
    parser = argparse.ArgumentParser(description="Strip steganographic PNG/JPEG headers and remux to MP4.")
    parser.add_argument("-i", "--input", required=True, help="Input video file path")
    parser.add_argument("-o", "--output", default="final_output.mp4", help="Output MP4 file path")
    args = parser.parse_args()

    input_file = args.input
    temporary_ts = "tmp.ts"
    final_output = args.output

    # Compilation of magic byte pattern for both PNG and JPEG starters
    # PNG: \x89PNG\r\n\x1a\n
    # JPEG: \xff\xd8\xff (Standard JPEG Start of Image marker)
    MASK_PATTERN = re.compile(b'(\x89PNG\r\n\x1a\n|\xff\xd8\xff)')

    print(f"[*] Processing {input_file}...")

    if not os.path.exists(input_file):
        print(f"[-] Error: {input_file} not found.")
        exit(1)

    with open(input_file, "rb") as infile, open(temporary_ts, "wb") as outfile:
        data = infile.read()
        
        # Split using regex pattern to catch whichever mask the server used
        fragments = MASK_PATTERN.split(data)
        written_fragments = 0
        
        for fragment in fragments:
            if not fragment: 
                continue
            
            # Since regex splitting can leave the matched patterns or empty segments, 
            # we skip standalone signature matches
            if fragment == b'\x89PNG\r\n\x1a\n' or fragment == b'\xff\xd8\xff':
                continue
                
            # Scan inside the fragment payload for the true video stream entry points
            id3_index = fragment.find(b'ID3')
            if id3_index != -1:
                outfile.write(fragment[id3_index:])
                written_fragments += 1
            else:
                g_index = fragment.find(b'G') # 0x47 MPEG-TS sync byte
                if g_index != -1:
                    outfile.write(fragment[g_index:])
                    written_fragments += 1

    print(f"[+] Done! Stripped {written_fragments} junk image wrappers. Saved temporary file as {temporary_ts}")
    print(f"[*] Remuxing {temporary_ts} to {final_output} via FFmpeg...")

    ffmpeg_command = ["ffmpeg", "-y", "-i", temporary_ts, "-c", "copy", "-bsf:a", "aac_adtstoasc", final_output]

    try:
        subprocess.run(ffmpeg_command, check=True)
        print(f"[+] Success! Cleaned video saved to {final_output}")
        if os.path.exists(temporary_ts):
            os.remove(temporary_ts)
            print(f"[*] Cleaned up temporary file: {temporary_ts}")
    except subprocess.CalledProcessError as e:
        print(f"[-] Error during FFmpeg remuxing: {e}")
    except FileNotFoundError:
        print("[-] Error: 'ffmpeg' command not found. Ensure FFmpeg is available in your PATH environment.")

if __name__ == "__main__":
    main()