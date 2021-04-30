
extern crate log;
extern crate serde_json;

use log::info;
use std::io::prelude::*;
use std::fs;

use crate::msbwt_core::*;

pub struct RleBWT {
    bwt: Vec<u8>,
    total_counts: [u64; VC_LEN],
    start_index: [u64; VC_LEN],
    end_index: [u64; VC_LEN],
    fm_index: [Vec<u64>; VC_LEN],
    ref_index: Vec<u64>,
    total_size: u64,
    bin_power: u8,
    bin_size: u64
}

impl Default for RleBWT {
    fn default() -> Self {
        let bin_power: u8 = 8;
        let bin_size: u64 = 0x1 << bin_power;
        Self {
            bwt: vec![],
            total_counts: [0; VC_LEN],
            start_index: [0; VC_LEN],
            end_index: [0; VC_LEN],
            fm_index: Default::default(),
            ref_index: Default::default(),
            total_size: 0,
            bin_power: bin_power,
            bin_size: bin_size
        }
    }
}

impl BWT for RleBWT {
    fn load_vector(&mut self, bwt: Vec<u8>) {
        //i am the captain now
        self.bwt = bwt;
        info!("Loading BWT from vector of length {}", self.bwt.len());
        
        //we copied it in, standard init now
        self.standard_init();
    }

    fn load_numpy_file(&mut self, filename: &str) -> std::io::Result<()> {
        //read the numpy header: http://docs.scipy.org/doc/numpy-1.10.1/neps/npy-format.html
        //get the initial file size
        let file_metadata: fs::Metadata = fs::metadata(&filename)?;
        let full_file_size: u64 = file_metadata.len();

        //read the initial fixed header
        let mut file = fs::File::open(&filename)?;
        let mut init_header: Vec<u8> = vec![0; 10];
        let read_count: usize = file.read(&mut init_header[..])?;
        if read_count != 10 {
            panic!("Could not read initial 10 bytes of header for file {:?}", filename);
        }

        //read the dynamic header
        let header_len: usize = init_header[8] as usize + 256 * init_header[9] as usize;
        let mut skip_bytes: usize = 10+header_len;
        if skip_bytes % 16 != 0 {
            skip_bytes = ((skip_bytes / 16)+1)*16;
        }
        let mut skip_header: Vec<u8> = vec![0; skip_bytes-10];
        match file.read_exact(&mut skip_header[..]) {
            Ok(()) => {},
            Err(e) => {
                return Err(
                    std::io::Error::new(
                        e.kind(),
                        format!("Could not read bytes 10-{:?} of header for file {:?}, root-error {:?}", skip_bytes, filename, e)
                    )
                );
            }
        }
        
        //parse the header string for the expected length, requires a lot of manipulation of the string because of numpy header styling
        let header_string = String::from_utf8(skip_header).unwrap()
            .replace("\'", "\"")
            .replace("False", "false")
            .replace("(", "[")
            .replace(")", "]")
            .replace(", }", "}")
            .replace(", ]", "]")
            .replace(",]", "]");
        let header_dict: serde_json::Value = serde_json::from_str(&header_string)
            .unwrap_or_else(|_| panic!("Error while parsing header string: {:?}", header_string));
        let expected_length: u64 = header_dict["shape"][0].as_u64().unwrap();
        
        //check that the disk size matches our expectation
        let bwt_disk_size: u64 = full_file_size - skip_bytes as u64;
        if expected_length != bwt_disk_size {
            return Err(
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("Header indicates shape of {:?}, but remaining file size is {:?}", expected_length, bwt_disk_size)
                )
            );
        }

        //finally read in everything else
        self.bwt = Vec::<u8>::with_capacity(bwt_disk_size as usize);//vec![0; bwt_disk_size as usize];
        let read_count: usize = file.read_to_end(&mut self.bwt)?;
        if read_count as u64 != bwt_disk_size {
            return Err(
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("Only read {:?} of {:?} bytes of BWT body for file {:?}", read_count, bwt_disk_size, filename)
                )
            );
        }
        info!("Loading BWT with {:?} compressed values", bwt_disk_size);

        //we loaded the file into memory, standard init now
        self.standard_init();

        Ok(())
    }

    #[inline]
    fn get_total_counts(&self, symbol: u8) -> u64 {
        self.total_counts[symbol as usize]
    }

    unsafe fn constrain_range(&self, sym: u8, input_range: &BWTRange) -> BWTRange {
        //first find the low value
        let bin_id: usize = (input_range.l >> self.bin_power) as usize;
        let mut compressed_index: usize = self.ref_index[bin_id] as usize;
        let mut bwt_index: u64 = 0;
        for x in 0..VC_LEN {
            bwt_index += self.fm_index[x][bin_id];
        }
        
        let mut ret: BWTRange = Default::default();
        ret.l = self.start_index[sym as usize]+self.fm_index[sym as usize][bin_id];
        
        let mut prev_char: u8 = 255;
        let mut current_char: u8;
        let mut prev_count: u64 = 0;
        let mut power_multiple: u64 = 1;
        
        while bwt_index+prev_count < input_range.l {
            current_char = self.bwt[compressed_index] & MASK;
            if current_char == prev_char {
                prev_count += (self.bwt[compressed_index] >> LETTER_BITS) as u64 * power_multiple;
                power_multiple *= NUM_POWER as u64;
            }
            else {
                if prev_char == sym {
                    ret.l += prev_count;
                }
                
                bwt_index += prev_count;
                prev_count = (self.bwt[compressed_index] >> LETTER_BITS) as u64;
                prev_char = current_char;
                power_multiple = NUM_POWER as u64;
            }
            compressed_index += 1;
        }

        let temp_offset: u64 = ret.l;
        if prev_char == sym {
            ret.l += input_range.l - bwt_index as u64;
        }

        //now find the high value
        let bin_id_h: usize = (input_range.h >> self.bin_power) as usize;
        if bin_id == bin_id_h {
            ret.h = temp_offset;
        }
        else {
            compressed_index = self.ref_index[bin_id_h] as usize;
            bwt_index = 0;
            for x in 0..VC_LEN {
                bwt_index += self.fm_index[x][bin_id_h];
            }
            
            ret.h = self.start_index[sym as usize]+self.fm_index[sym as usize][bin_id_h];
            
            prev_char = 255;
            prev_count = 0;
            power_multiple = 1;
        }
        
        while bwt_index+prev_count < input_range.h {
            current_char = self.bwt[compressed_index] & MASK;
            if current_char == prev_char {
                prev_count += (self.bwt[compressed_index] >> LETTER_BITS) as u64 * power_multiple;
                power_multiple *= NUM_POWER as u64;
            }
            else {
                if prev_char == sym {
                    ret.h += prev_count;
                }
                
                bwt_index += prev_count;
                prev_count = (self.bwt[compressed_index] >> LETTER_BITS) as u64;
                prev_char = current_char;
                power_multiple = NUM_POWER as u64;
            }
            compressed_index += 1;
        }
        
        if prev_char == sym {
            ret.h += input_range.h - bwt_index;
        }
        ret
    }
}

impl RleBWT {
    /// Allocation function for the BWT, look at `load_vector(...)` for initialization.
    /// # Examples
    /// ```rust
    /// use msbwt::rle_bwt::RleBWT;
    /// let mut bwt: RleBWT = RleBWT::new();
    /// ```
    pub fn new() -> Self {
        RleBWT::with_bin_power(8)
    }

    /// Allocation function for the BWT, look at `load_vector(...)` for initialization.
    /// # Examples
    /// ```rust
    /// use msbwt::rle_bwt::RleBWT;
    /// let mut bwt: RleBWT = RleBWT::new();
    /// ```
    pub fn with_bin_power(bin_power: u8) -> Self {
        let bin_size: u64 = 0x1 << bin_power;
        Self {
            bwt: vec![],
            total_counts: [0; VC_LEN],
            start_index: [0; VC_LEN],
            end_index: [0; VC_LEN],
            fm_index: Default::default(),
            ref_index: Default::default(),
            total_size: 0,
            bin_power: bin_power,
            bin_size: bin_size
        }
    }

    fn standard_init(&mut self) {
        //we will call this function when the bwt is fully loaded into memory
        //first pass does a count so we can pre-allocate the indices correctly
        self.calculate_totals();

        //now we can construct the FM-index pieces in the binary storage format for rapid speed
        self.construct_fmindex();

        /*
        //now do the fixed initialization
        let full_range: BWTRange = BWTRange {
            l: 0,
            h: self.total_size
        };
        unsafe {
            self.fixed_init[0] = self.constrain_range(1, &full_range);
            self.fixed_init[1] = self.constrain_range(2, &full_range);
            self.fixed_init[2] = self.constrain_range(3, &full_range);
            self.fixed_init[3] = self.constrain_range(5, &full_range);
        }

        self.populate_cache(false);
        */
        info!("Finished BWT initialization.");
    }

    /// This calculates the total number of each symbol in the BWT from the compressed representation.
    /// Running this is part of initialization and is a pre-requisite for FM-index construction.
    fn calculate_totals(&mut self) {
        let mut prev_char: u8 = 255;
        let mut current_char: u8;
        let mut power_multiple: u64 = 1;
        let mut current_count: u64;

        //go through each compressed block in the RLE encoded vector to calculate total character counts
        self.total_counts = [0; VC_LEN];
        for value in &self.bwt {
            current_char = value & MASK;
            if current_char == prev_char {
                power_multiple *= NUM_POWER as u64;
            }
            else {
                power_multiple = 1;
            }
            prev_char = current_char;
            current_count = (value >> LETTER_BITS) as u64 * power_multiple;
            self.total_counts[current_char as usize] += current_count;
        }

        //calculate start/end indices from the total
        self.start_index = [0; VC_LEN];
        self.end_index = [0; VC_LEN];
        let mut sum_offset: u64 = 0;
        for i in 0..VC_LEN {
            self.start_index[i] = sum_offset;
            sum_offset += self.total_counts[i];
            self.end_index[i] = sum_offset;
        }
        self.total_size = self.end_index[VC_LEN-1];
        info!("Loaded BWT with symbol counts: {:?}", self.total_counts);
    }

    /// This will create the actual indexing structure. For RLE, it's just a sampled index
    fn construct_fmindex(&mut self) {
        //uint64_t samplingSize = (uint64_t)ceil(((float)this->totalSize+1)/this->binSize)+1;
        //we have an extra +1 up there^; do we need that for some reason?
        let index_length: usize = ((self.total_size as f64) / (self.bin_size as f64)).ceil() as usize + 1;
        for subvec in self.fm_index.iter_mut() {
            subvec.resize(index_length, 0);
        }
        self.ref_index.resize(index_length, 0);

        //initial indices start at the start_index offsets
        //let mut current_index: [u64; VC_LEN] = self.start_index.clone();
        //initialize to all zeros
        let mut current_index: [u64; VC_LEN] = [0; VC_LEN];

        //decided to just set these all to usize since many are indices, seems relatively safe
        let mut total_char_count: usize = 0;
        let mut power_multiple: usize = 1;
        let mut bin_end: usize = 0;
        let mut bin_id: usize = 0;
        let mut bwt_index: usize = 0;
        let mut prev_start: usize = 0;
        let mut prev_char: u8 = 0;
        let mut current_char: u8;

        //go through each run in the BWT and set FM-indices as we go
        for (x, byte_value) in self.bwt.iter().enumerate() {
            current_char = byte_value & MASK;
            if current_char == prev_char {
                total_char_count += (byte_value >> LETTER_BITS) as usize * power_multiple;
                power_multiple *= NUM_POWER;
            }
            else {
                //first save the current FM-index entry
                //while bwt_index+total_char_count >= bin_end {
                while bwt_index+total_char_count > bin_end {
                    self.ref_index[bin_id] = prev_start as u64;
                    for y in 0..VC_LEN {
                        self.fm_index[y][bin_id] = current_index[y];
                    }
                    bin_end += self.bin_size as usize;
                    bin_id += 1;
                }
                
                //now add the previous
                current_index[prev_char as usize] += total_char_count as u64;
                bwt_index += total_char_count;
                
                prev_char = current_char;
                prev_start = x;
                total_char_count = (byte_value >> LETTER_BITS) as usize;
                power_multiple = NUM_POWER;
            }
        }
        
        //fill out the remaining entries
        //while bwt_index+total_char_count >= bin_end {
        while bwt_index+total_char_count > bin_end {
            self.ref_index[bin_id] = prev_start as u64;
            for y in 0..VC_LEN {
                self.fm_index[y][bin_id] = current_index[y];
            }
            bin_end += self.bin_size as usize;
            bin_id += 1;
        }
        
        //set the last entry
        current_index[prev_char as usize] += total_char_count as u64;//forces countSoFar to hold the very end FM-index entry
        self.ref_index[index_length-1] = self.bwt.len() as u64; //need to point to the index at the end
        for y in 0..VC_LEN {
            self.fm_index[y][index_length-1] = current_index[y];
        }
        /*
        //calculate the total offset_sum
        self.offset_sum = 0;
        for y in 0..VC_LEN {
            self.offset_sum += self.fm_index[y][0];
        }
        */
        //next steps are to write tests for index construction
        //we should make the index size a parameter for testing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bwt_converter::*;
    use flate2::{Compression, GzBuilder};
    //use std::io::Cursor;
    use tempfile::{Builder, NamedTempFile};
    
    fn naive_bwt(inputs: &Vec<&str>) -> String {
        let mut rotations: Vec<String> = vec![];
        for s in inputs.iter() {
            let dollar_string = s.to_string()+&"$".to_string();
            for l in 0..dollar_string.len() {
                rotations.push(dollar_string[l..].to_string()+&dollar_string[..l]);
            }
        }
        rotations.sort();
        let mut ret: String = String::with_capacity(rotations.len());
        for r in rotations.iter() {
            ret.push(r.as_bytes()[r.len()-1] as char);
        }
        ret
    }

    fn write_strings_to_fqgz(data: Vec<&str>) -> NamedTempFile {
        let file: NamedTempFile = Builder::new().prefix("temp_data_").suffix(".fq.gz").tempfile().unwrap();
        let mut gz = GzBuilder::new().write(file, Compression::default());
        let mut i: usize = 0;
        for s in data {
            writeln!(gz, "@seq_{}\n{}\n+\n{}", i, s, "F".repeat(s.len())).unwrap();
            i += 1;
        }

        //have to keep the file handle or everything blows up
        gz.finish().unwrap()
    }

    #[test]
    fn test_load_rlebwt_from_npy() {
        //strings - "CCGT\nACG\nN"
        //build the BWT
        let data: Vec<&str> = vec!["CCGT", "N", "ACG"];
        
        //stream and compress the BWT
        //let bwt_stream = stream_bwt_from_fastqs(&fastq_filenames).unwrap();
        let bwt_stream = naive_bwt(&data);
        let compressed_bwt = convert_to_vec(bwt_stream.as_bytes());
        
        //save the output to a temporary numpy file
        let bwt_file: NamedTempFile = Builder::new().prefix("temp_data_").suffix(".npy").tempfile().unwrap();
        let filename: String = bwt_file.path().to_str().unwrap().to_string();
        save_bwt_numpy(&compressed_bwt[..], &filename).unwrap();
        
        //load it back in and verify counts
        let mut bwt = RleBWT::new();
        bwt.load_numpy_file(&filename).unwrap();

        let expected_totals = vec![3, 1, 3, 2, 1, 1];
        for i in 0..6 {
            //make sure the total counts are correct
            assert_eq!(bwt.get_total_counts(i as u8), expected_totals[i]);
        }
    }

    #[test]
    fn test_fmindex() {
        //strings - "CCGT\nACG\nN"
        //build the BWT
        let data: Vec<&str> = vec!["CCGT", "N", "ACG"];
        
        //stream and compress the BWT
        //let bwt_stream = stream_bwt_from_fastqs(&fastq_filenames).unwrap();
        let bwt_stream = naive_bwt(&data);
        assert_eq!(bwt_stream, "GTN$$ACCC$G");
        let compressed_bwt = convert_to_vec(bwt_stream.as_bytes());
        //[G, T, N, 2$, A, 3C, $, G]
        assert_eq!(compressed_bwt.len(), 8);
        
        //load it back in and verify counts
        for bin_power in 1..5 {
            let mut bwt = RleBWT::with_bin_power(bin_power);
            bwt.load_vector(compressed_bwt.clone());

            let expected_totals = vec![3, 1, 3, 2, 1, 1];
            for i in 0..6 {
                //make sure the total counts are correct
                assert_eq!(bwt.get_total_counts(i as u8), expected_totals[i]);
            }
            
            //make sure the chunk sizes are as expected
            let expected_length: usize = (bwt_stream.len() as f64 / (0x1 << bin_power) as f64).ceil() as usize+1;
            assert_eq!(bwt.ref_index.len(), expected_length);
            for sym in 0..6 {
                assert_eq!(bwt.fm_index[sym].len(), expected_length);
            }

            if bin_power == 1 {
                //[G, T, N, 2$, A, 3C, $, G]
                //every 2 bases should get an entry
                let expected_ref: Vec<u64> = vec![0, 2, 3, 5, 5, 7, 8];
                assert_eq!(bwt.ref_index, expected_ref);
                let expected_fm_index: [Vec<u64>; VC_LEN] = [
                    vec![0, 0, 0, 2, 2, 3, 3],
                    vec![0, 0, 0, 1, 1, 1, 1],
                    vec![0, 0, 0, 0, 0, 3, 3],
                    vec![0, 1, 1, 1, 1, 1, 2],
                    vec![0, 0, 1, 1, 1, 1, 1],
                    vec![0, 1, 1, 1, 1, 1, 1]
                ];
                for i in 0..VC_LEN {
                    assert_eq!(bwt.fm_index[i], expected_fm_index[i]);
                }
            } 
            /*else if bin_power == 2 {
                //every 4 bases should get an entry
                let expected_ref: Vec<u64> = vec![0, 3, 5, 8];
                assert_eq!(bwt.ref_index, expected_ref);
                let expected_fm_index: [Vec<u64>; VC_LEN] = [
                    vec![0, 0, 2, 3],
                    vec![3, 3, 4, 4],
                    vec![4, 4, 4, 7],
                    vec![7, 8, 8, 9],
                    vec![9, 10, 10, 10],
                    vec![10, 11, 11, 11]
                ];
                for i in 0..VC_LEN {
                    assert_eq!(bwt.fm_index[i], expected_fm_index[i]);
                }
            } else if bin_power == 3 {
                //every 8 bases should get an entry
                let expected_ref: Vec<u64> = vec![0, 5, 8];
                assert_eq!(bwt.ref_index, expected_ref);
                let expected_fm_index: [Vec<u64>; VC_LEN] = [
                    vec![0, 2, 3],
                    vec![3, 4, 4],
                    vec![4, 4, 7],
                    vec![7, 8, 9],
                    vec![9, 10, 10],
                    vec![10, 11, 11]
                ];
                for i in 0..VC_LEN {
                    assert_eq!(bwt.fm_index[i], expected_fm_index[i]);
                }
            } else if bin_power == 4 {
                //every 16 bases should get an entry
                let expected_ref: Vec<u64> = vec![0, 8];
                assert_eq!(bwt.ref_index, expected_ref);
                let expected_fm_index: [Vec<u64>; VC_LEN] = [
                    vec![0, 3],
                    vec![3, 4],
                    vec![4, 7],
                    vec![7, 9],
                    vec![9, 10],
                    vec![10, 11]
                ];
                for i in 0..VC_LEN {
                    assert_eq!(bwt.fm_index[i], expected_fm_index[i]);
                }
            }*/
        }
    }

    #[test]
    fn test_constrain_range() {
        //strings - "CCGT\nACG\nN"
        //build the BWT
        let data: Vec<&str> = vec!["CCGT", "N", "ACG"];
        
        //stream and compress the BWT
        //let bwt_stream = stream_bwt_from_fastqs(&fastq_filenames).unwrap();
        let bwt_stream = naive_bwt(&data);
        assert_eq!(bwt_stream, "GTN$$ACCC$G");
        let compressed_bwt = convert_to_vec(bwt_stream.as_bytes());
        //[G, T, N, 2$, A, 3C, $, G]
        assert_eq!(compressed_bwt.len(), 8);
        
        //load it back in and verify counts
        for bin_power in 1..5 {
            let mut bwt = RleBWT::with_bin_power(bin_power);
            bwt.load_vector(compressed_bwt.clone());

            let initial_range = BWTRange {
                l: 0,
                h: bwt_stream.len() as u64
            };
            
            //this is verifying that all single-symbol queries get the start/end range
            for sym in 0..VC_LEN {
                let new_range = unsafe {
                    bwt.constrain_range(sym as u8, &initial_range)
                };
                assert_eq!(new_range, BWTRange{l: bwt.start_index[sym], h: bwt.end_index[sym]});
            }

            //now lets verify that we get all ascending symbols
        }
    }
}